//! Slash commands, autocomplete and all button / select-menu handling.

use crate::{
    player::{AppState, EnqueueResult, LoopMode, SearchSession},
    search::Provider,
    ui::{self, Tone},
    util,
};
use anyhow::{anyhow, Result};
use serenity::{builder::*, client::Context, model::prelude::*};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Instant,
};
use tracing::warn;

pub async fn dispatch(ctx: Context, interaction: Interaction, app: Arc<AppState>) {
    match interaction {
        Interaction::Command(command) => {
            if let Err(e) = handle_command(&ctx, &command, &app).await {
                warn!("/{} failed: {e:#}", command.data.name);
                report_command_error(&ctx, &command, &e).await;
            }
        }
        Interaction::Autocomplete(command) => {
            if let Err(e) = handle_autocomplete(&ctx, &command, &app).await {
                warn!("autocomplete failed: {e:#}");
            }
        }
        Interaction::Component(component) => {
            if let Err(e) = handle_component(&ctx, &component, &app).await {
                warn!("component {} failed: {e:#}", component.data.custom_id);
                report_component_error(&ctx, &component, &e).await;
            }
        }
        _ => {}
    }
}

// ── helpers ────────────────────────────────────────────────────────────────

fn opt_str(cmd: &CommandInteraction, name: &str) -> Option<String> {
    let options = cmd.data.options();
    options
        .iter()
        .find(|o| o.name == name)
        .and_then(|o| match &o.value {
            ResolvedValue::String(s) => Some((*s).to_owned()),
            ResolvedValue::Integer(i) => Some(i.to_string()),
            _ => None,
        })
}

fn opt_int(cmd: &CommandInteraction, name: &str) -> Option<i64> {
    let options = cmd.data.options();
    options
        .iter()
        .find(|o| o.name == name)
        .and_then(|o| match &o.value {
            ResolvedValue::Integer(i) => Some(*i),
            _ => None,
        })
}

fn opt_bool(cmd: &CommandInteraction, name: &str) -> Option<bool> {
    let options = cmd.data.options();
    options
        .iter()
        .find(|o| o.name == name)
        .and_then(|o| match &o.value {
            ResolvedValue::Boolean(b) => Some(*b),
            _ => None,
        })
}

fn guild_id(cmd: &CommandInteraction) -> Result<GuildId> {
    cmd.guild_id
        .ok_or_else(|| anyhow!("Nexcord only works inside a server"))
}

async fn finish(ctx: &Context, cmd: &CommandInteraction, embed: CreateEmbed) -> Result<()> {
    cmd.edit_response(ctx, EditInteractionResponse::new().embed(embed))
        .await?;
    Ok(())
}

async fn finish_with(
    ctx: &Context,
    cmd: &CommandInteraction,
    embed: CreateEmbed,
    components: Vec<CreateActionRow>,
) -> Result<()> {
    cmd.edit_response(
        ctx,
        EditInteractionResponse::new()
            .embed(embed)
            .components(components),
    )
    .await?;
    Ok(())
}

fn added_notice(result: &EnqueueResult) -> CreateEmbed {
    let mut embed = ui::added_embed(&result.first, result.position);
    if result.added > 1 {
        embed = embed.field(
            "PLAYLIST",
            format!("**{}** tracks were added to the queue.", result.added),
            false,
        );
    }
    embed.field(
        "TIP",
        "Not the right one? Use `/search` or `/play pick:true` to choose an exact result.",
        false,
    )
}

async fn report_command_error(ctx: &Context, cmd: &CommandInteraction, e: &anyhow::Error) {
    let embed = ui::notice("Something went wrong", &e.to_string(), Tone::Bad);
    if cmd
        .edit_response(ctx, EditInteractionResponse::new().embed(embed.clone()))
        .await
        .is_ok()
    {
        return;
    }
    let _ = cmd
        .create_response(
            ctx,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .ephemeral(true)
                    .embed(embed),
            ),
        )
        .await;
}

async fn report_component_error(ctx: &Context, comp: &ComponentInteraction, e: &anyhow::Error) {
    let embed = ui::notice("Action failed", &e.to_string(), Tone::Bad);
    if comp
        .edit_response(ctx, EditInteractionResponse::new().embed(embed.clone()))
        .await
        .is_ok()
    {
        return;
    }
    let _ = comp
        .create_followup(
            ctx,
            CreateInteractionResponseFollowup::new()
                .ephemeral(true)
                .embed(embed),
        )
        .await;
}

// ── slash commands ─────────────────────────────────────────────────────────

async fn handle_command(
    ctx: &Context,
    cmd: &CommandInteraction,
    app: &Arc<AppState>,
) -> Result<()> {
    // Defer first: searching, joining voice and posting the panel can exceed 3 s.
    cmd.defer_ephemeral(ctx).await?;
    let requester = cmd.user.name.clone();

    match cmd.data.name.as_str() {
        "play" => {
            let guild = guild_id(cmd)?;
            let query = opt_str(cmd, "query").ok_or_else(|| anyhow!("missing query"))?;
            let provider = Provider::parse(&opt_str(cmd, "source").unwrap_or_default());
            let pick = opt_bool(cmd, "pick").unwrap_or(false);

            if util::looks_like_url(&query) {
                // Autocomplete hands us exact URLs, and pasted links work directly.
                let tracks = app
                    .searcher
                    .resolve(&query, &requester, app.config.max_queue)
                    .await?;
                let result = AppState::enqueue(
                    app,
                    ctx,
                    guild,
                    cmd.user.id,
                    &requester,
                    tracks,
                    cmd.channel_id,
                )
                .await?;
                finish(ctx, cmd, added_notice(&result)).await
            } else if pick {
                open_picker(ctx, cmd, app, guild, &query, provider).await
            } else {
                let mut tracks = app.searcher.search(&query, provider).await?;
                let best = tracks.remove(0);
                let result = AppState::enqueue(
                    app,
                    ctx,
                    guild,
                    cmd.user.id,
                    &requester,
                    vec![best],
                    cmd.channel_id,
                )
                .await?;
                finish(ctx, cmd, added_notice(&result)).await
            }
        }

        "search" => {
            let guild = guild_id(cmd)?;
            let query = opt_str(cmd, "query").ok_or_else(|| anyhow!("missing query"))?;
            let provider = Provider::parse(&opt_str(cmd, "source").unwrap_or_default());
            if util::looks_like_url(&query) {
                let tracks = app
                    .searcher
                    .resolve(&query, &requester, app.config.max_queue)
                    .await?;
                let result = AppState::enqueue(
                    app,
                    ctx,
                    guild,
                    cmd.user.id,
                    &requester,
                    tracks,
                    cmd.channel_id,
                )
                .await?;
                finish(ctx, cmd, added_notice(&result)).await
            } else {
                open_picker(ctx, cmd, app, guild, &query, provider).await
            }
        }

        "panel" => {
            let guild = guild_id(cmd)?;
            app.set_text_channel(guild, cmd.channel_id).await;
            app.reset_panel(guild).await;
            finish(
                ctx,
                cmd,
                ui::notice(
                    "Panel ready",
                    "The control panel was posted in this channel.",
                    Tone::Good,
                ),
            )
            .await
        }

        "queue" => {
            let guild = guild_id(cmd)?;
            let snap = app.snapshot(guild).await;
            let pages = ui::queue_pages(snap.queue_len);
            let page = opt_int(cmd, "page")
                .map(|p| (p.max(1) as usize - 1).min(pages.saturating_sub(1)))
                .unwrap_or_else(|| 0);
            app.set_queue_page(guild, page).await;
            finish_with(
                ctx,
                cmd,
                ui::queue_embed(&snap, page, pages),
                ui::queue_components(page, pages),
            )
            .await
        }

        "now" => {
            let guild = guild_id(cmd)?;
            let snap = app.snapshot(guild).await;
            finish(ctx, cmd, ui::now_embed(&snap)).await
        }

        "pause" => {
            let guild = guild_id(cmd)?;
            let paused = app.toggle_pause(guild).await?;
            finish(
                ctx,
                cmd,
                ui::notice(
                    "Playback",
                    if paused {
                        "⏸ Paused."
                    } else {
                        "▶️ Resumed."
                    },
                    Tone::Good,
                ),
            )
            .await
        }

        "skip" => {
            let guild = guild_id(cmd)?;
            let to = opt_int(cmd, "to").map(|v| v.max(1) as usize);
            app.skip(guild, to).await?;
            finish(
                ctx,
                cmd,
                ui::notice("Skipped", "On to the next track.", Tone::Good),
            )
            .await
        }

        "stop" => {
            let guild = guild_id(cmd)?;
            app.stop(guild).await?;
            finish(
                ctx,
                cmd,
                ui::notice(
                    "Stopped",
                    "Playback stopped and the queue was cleared.",
                    Tone::Neutral,
                ),
            )
            .await
        }

        "seek" => {
            let guild = guild_id(cmd)?;
            let input = opt_str(cmd, "position").ok_or_else(|| anyhow!("missing position"))?;
            let position = util::parse_time(&input)
                .ok_or_else(|| anyhow!("use a time like `1:23`, `01:02:03` or `90`"))?;
            app.seek(guild, position).await?;
            finish(
                ctx,
                cmd,
                ui::notice(
                    "Seeked",
                    &format!("Jumped to `{}`.", util::fmt_duration(position)),
                    Tone::Good,
                ),
            )
            .await
        }

        "volume" => {
            let guild = guild_id(cmd)?;
            let percent = opt_int(cmd, "percent").unwrap_or(100).clamp(0, 200) as u8;
            let value = app.set_volume(guild, percent).await?;
            finish(
                ctx,
                cmd,
                ui::notice(
                    "Volume",
                    &format!("Set to **{}%**.", (value * 100.0).round() as i32),
                    Tone::Good,
                ),
            )
            .await
        }

        "loop" => {
            let guild = guild_id(cmd)?;
            let mode = LoopMode::parse(&opt_str(cmd, "mode").unwrap_or_default());
            app.set_loop(guild, mode).await?;
            finish(
                ctx,
                cmd,
                ui::notice(
                    "Loop",
                    &format!("Loop mode is now **{}**.", mode.label()),
                    Tone::Good,
                ),
            )
            .await
        }

        "shuffle" => {
            let guild = guild_id(cmd)?;
            let count = app.shuffle(guild).await?;
            finish(
                ctx,
                cmd,
                ui::notice(
                    "Shuffled",
                    &format!("Reordered **{count}** upcoming tracks."),
                    Tone::Good,
                ),
            )
            .await
        }

        "remove" => {
            let guild = guild_id(cmd)?;
            let position = opt_int(cmd, "position").unwrap_or(1).max(1) as usize;
            let removed = app.remove_at(guild, position).await?;
            finish(
                ctx,
                cmd,
                ui::notice(
                    "Removed",
                    &format!(
                        "Removed **{}** from the queue.",
                        util::clean_md(&removed.title)
                    ),
                    Tone::Good,
                ),
            )
            .await
        }

        "join" => {
            let guild = guild_id(cmd)?;
            let channel = AppState::ensure_voice(app, ctx, guild, cmd.user.id).await?;
            app.set_text_channel(guild, cmd.channel_id).await;
            app.refresh_panel(guild).await;
            finish(
                ctx,
                cmd,
                ui::notice(
                    "Connected",
                    &format!("Joined <#{}>.", channel.get()),
                    Tone::Good,
                ),
            )
            .await
        }

        "leave" => {
            let guild = guild_id(cmd)?;
            app.leave(guild).await?;
            finish(
                ctx,
                cmd,
                ui::notice("Disconnected", "Left voice.", Tone::Neutral),
            )
            .await
        }

        "stats" => finish(ctx, cmd, ui::stats_embed(app.stats().await)).await,
        "help" => finish(ctx, cmd, ui::help_embed()).await,
        other => {
            finish(
                ctx,
                cmd,
                ui::notice(
                    "Unknown command",
                    &format!("`/{other}` isn't registered."),
                    Tone::Warn,
                ),
            )
            .await
        }
    }
}

async fn open_picker(
    ctx: &Context,
    cmd: &CommandInteraction,
    app: &Arc<AppState>,
    guild: GuildId,
    query: &str,
    provider: Provider,
) -> Result<()> {
    let tracks = app.searcher.search(query, provider).await?;
    let session = app.new_session(SearchSession {
        at: Instant::now(),
        guild_id: guild,
        user_id: cmd.user.id,
        text_channel: cmd.channel_id,
        tracks: tracks.clone(),
    });
    finish_with(
        ctx,
        cmd,
        ui::search_embed(query, &tracks, provider),
        ui::search_components(&session, &tracks),
    )
    .await
}

// ── autocomplete ───────────────────────────────────────────────────────────

/// Never blocks on yt-dlp. Returns whatever is already cached and, on a miss,
/// warms the cache in a bounded background task for the next keystroke.
/// Choosing a suggestion submits its exact URL, so `/play` resolves that track
/// without another fuzzy search.
async fn handle_autocomplete(
    ctx: &Context,
    cmd: &CommandInteraction,
    app: &Arc<AppState>,
) -> Result<()> {
    let options = cmd.data.options();
    let focused = options
        .iter()
        .find_map(|o| match &o.value {
            ResolvedValue::Autocomplete { value, .. } => Some((*value).to_owned()),
            _ => None,
        })
        .unwrap_or_default();
    let focused = focused.trim();

    let mut response = CreateAutocompleteResponse::new();
    if focused.chars().count() >= 2 && !util::looks_like_url(focused) {
        match app.searcher.cached(focused, Provider::Auto) {
            Some(tracks) => {
                for track in tracks.iter().take(10) {
                    let name = util::truncate(
                        &format!(
                            "{} · {} · {}",
                            track.title,
                            track.author,
                            util::fmt_secs(track.duration)
                        ),
                        100,
                    );
                    let value = if track.url.len() <= 100 {
                        track.url.clone()
                    } else {
                        util::truncate(&track.title, 100)
                    };
                    response = response.add_string_choice(name, value);
                }
            }
            None if app.inflight.load(Ordering::Relaxed) < 2 => {
                app.inflight.fetch_add(1, Ordering::Relaxed);
                let warm: Arc<AppState> = Arc::clone(app);
                let query = focused.to_owned();
                tokio::spawn(async move {
                    let _ = warm.searcher.search(&query, Provider::Auto).await;
                    warm.inflight.fetch_sub(1, Ordering::Relaxed);
                });
            }
            None => {}
        }
    }

    cmd.create_response(ctx, CreateInteractionResponse::Autocomplete(response))
        .await?;
    Ok(())
}

// ── components ─────────────────────────────────────────────────────────────

fn is_control(id: &str) -> bool {
    matches!(
        id,
        "nx:playpause"
            | "nx:skip"
            | "nx:stop"
            | "nx:loop"
            | "nx:shuffle"
            | "nx:voldown"
            | "nx:volup"
            | "nx:back"
    )
}

async fn voice_allowed(ctx: &Context, app: &Arc<AppState>, guild: GuildId, user: UserId) -> bool {
    let bot_channel = app.guild(guild).await.lock().await.voice_channel;
    let Some(bot_channel) = bot_channel else {
        return true;
    };
    let user_channel = ctx
        .cache
        .guild(guild)
        .and_then(|g| g.voice_states.get(&user).and_then(|v| v.channel_id));
    user_channel == Some(bot_channel)
}

async fn handle_component(
    ctx: &Context,
    comp: &ComponentInteraction,
    app: &Arc<AppState>,
) -> Result<()> {
    let id = comp.data.custom_id.as_str();
    let Some(guild) = comp.guild_id else {
        comp.create_response(
            ctx,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .ephemeral(true)
                    .embed(ui::notice(
                        "Servers only",
                        "Music controls need a server.",
                        Tone::Warn,
                    )),
            ),
        )
        .await?;
        return Ok(());
    };

    if is_control(id) && !voice_allowed(ctx, app, guild, comp.user.id).await {
        comp.create_response(
            ctx,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .ephemeral(true)
                    .embed(ui::notice(
                        "Join the voice channel",
                        "Hop into the same voice channel as Nexcord to use these controls.",
                        Tone::Warn,
                    )),
            ),
        )
        .await?;
        return Ok(());
    }

    // These paths answer by editing the component's own message — no deferral.
    match id {
        "nx:pick" => return handle_pick(ctx, comp, app, guild, None).await,
        "nx:cancel" => {
            return close_picker(
                ctx,
                comp,
                ui::notice("Cancelled", "No track was added.", Tone::Neutral),
            )
            .await;
        }
        "nx:qprev" | "nx:qnext" | "nx:qclose" => {
            return handle_queue_button(ctx, comp, app, guild, id).await
        }
        _ if id.starts_with("nx:top:") => {
            // strip_once, not trim_start_matches: the latter strips *repeated*
            // prefixes and would silently corrupt a crafted custom_id.
            let session = id.strip_prefix("nx:top:").unwrap_or_default().to_owned();
            return handle_pick(ctx, comp, app, guild, Some(session)).await;
        }
        _ => {}
    }

    // Playback controls: acknowledge immediately, then do the work.
    comp.defer_ephemeral(ctx).await?;

    if id == "nx:queue" {
        let snap = app.snapshot(guild).await;
        let pages = ui::queue_pages(snap.queue_len);
        let page = app.queue_page(guild).await.min(pages.saturating_sub(1));
        comp.edit_response(
            ctx,
            EditInteractionResponse::new()
                .embed(ui::queue_embed(&snap, page, pages))
                .components(ui::queue_components(page, pages)),
        )
        .await?;
        return Ok(());
    }

    let embed = match id {
        "nx:playpause" => {
            let paused = app.toggle_pause(guild).await?;
            ui::notice(
                "Playback",
                if paused {
                    "⏸ Paused."
                } else {
                    "▶️ Resumed."
                },
                Tone::Good,
            )
        }
        "nx:skip" => {
            app.skip(guild, None).await?;
            ui::notice("Skipped", "On to the next track.", Tone::Good)
        }
        "nx:stop" => {
            app.stop(guild).await?;
            ui::notice(
                "Stopped",
                "Playback stopped and the queue was cleared.",
                Tone::Neutral,
            )
        }
        "nx:loop" => {
            let mode = app.cycle_loop(guild).await?;
            ui::notice(
                "Loop",
                &format!("Loop is now **{}**.", mode.label()),
                Tone::Good,
            )
        }
        "nx:shuffle" => {
            let count = app.shuffle(guild).await?;
            ui::notice(
                "Shuffled",
                &format!("Reordered **{count}** tracks."),
                Tone::Good,
            )
        }
        "nx:voldown" => {
            let value = app.nudge_volume(guild, -10).await?;
            ui::notice(
                "Volume",
                &format!("**{}%**", (value * 100.0).round() as i32),
                Tone::Good,
            )
        }
        "nx:volup" => {
            let value = app.nudge_volume(guild, 10).await?;
            ui::notice(
                "Volume",
                &format!("**{}%**", (value * 100.0).round() as i32),
                Tone::Good,
            )
        }
        "nx:back" => {
            app.seek_relative(guild, -15).await?;
            ui::notice("Seeked", "Rewound 15 seconds.", Tone::Good)
        }
        "nx:refresh" => {
            app.refresh_panel(guild).await;
            ui::notice("Refreshed", "The panel is up to date.", Tone::Good)
        }
        _ => ui::notice(
            "Unknown control",
            "That button isn't recognised.",
            Tone::Warn,
        ),
    };

    comp.edit_response(ctx, EditInteractionResponse::new().embed(embed))
        .await?;
    Ok(())
}

async fn close_picker(
    ctx: &Context,
    comp: &ComponentInteraction,
    embed: CreateEmbed,
) -> Result<()> {
    comp.create_response(
        ctx,
        CreateInteractionResponse::UpdateMessage(
            CreateInteractionResponseMessage::new()
                .embed(embed)
                .components(vec![]),
        ),
    )
    .await?;
    Ok(())
}

/// A search picker selection. Respond instantly, then enqueue in a detached
/// task (joining voice may take a few seconds) and report back with a followup.
async fn handle_pick(
    ctx: &Context,
    comp: &ComponentInteraction,
    app: &Arc<AppState>,
    guild: GuildId,
    forced_session: Option<String>,
) -> Result<()> {
    let (session_id, index) = match forced_session {
        Some(id) => (id, 0usize),
        None => {
            let values = match &comp.data.kind {
                ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => Vec::new(),
            };
            let raw = values.first().cloned().unwrap_or_default();
            match raw.rsplit_once(':') {
                Some((sid, idx)) => (sid.to_owned(), idx.parse::<usize>().unwrap_or(0)),
                None => (String::new(), 0),
            }
        }
    };
    let session = app
        .take_session(&session_id)
        .filter(|s| s.guild_id == guild);

    close_picker(
        ctx,
        comp,
        ui::notice(
            "One moment…",
            "Adding your track to the queue.",
            Tone::Neutral,
        ),
    )
    .await?;

    let ctx: Context = (*ctx).clone();
    let app: Arc<AppState> = (*app).clone();
    let comp: ComponentInteraction = (*comp).clone();
    let requester = comp.user.name.clone();
    tokio::spawn(async move {
        let embed = match session {
            Some(session) => match session.tracks.get(index).cloned() {
                Some(track) => {
                    match AppState::enqueue(
                        &app,
                        &ctx,
                        guild,
                        session.user_id,
                        &requester,
                        vec![track],
                        session.text_channel,
                    )
                    .await
                    {
                        Ok(result) => ui::added_embed(&result.first, result.position),
                        Err(e) => ui::notice("Couldn't add that track", &e.to_string(), Tone::Bad),
                    }
                }
                None => ui::notice(
                    "Invalid selection",
                    "That result no longer exists.",
                    Tone::Bad,
                ),
            },
            None => ui::notice(
                "Picker expired",
                "Run `/search` again — pickers last 10 minutes.",
                Tone::Warn,
            ),
        };
        let _ = comp
            .create_followup(
                &ctx,
                CreateInteractionResponseFollowup::new()
                    .ephemeral(true)
                    .embed(embed),
            )
            .await;
    });
    Ok(())
}

async fn handle_queue_button(
    ctx: &Context,
    comp: &ComponentInteraction,
    app: &Arc<AppState>,
    guild: GuildId,
    id: &str,
) -> Result<()> {
    if id == "nx:qclose" {
        return close_picker(
            ctx,
            comp,
            ui::notice(
                "Queue closed",
                "Use `/queue` or the 📜 button anytime.",
                Tone::Neutral,
            ),
        )
        .await;
    }
    let snap = app.snapshot(guild).await;
    let pages = ui::queue_pages(snap.queue_len);
    let mut page = app.queue_page(guild).await;
    if id == "nx:qprev" {
        page = page.saturating_sub(1);
    } else {
        page = (page + 1).min(pages.saturating_sub(1));
    }
    app.set_queue_page(guild, page).await;
    comp.create_response(
        ctx,
        CreateInteractionResponse::UpdateMessage(
            CreateInteractionResponseMessage::new()
                .embed(ui::queue_embed(&snap, page, pages))
                .components(ui::queue_components(page, pages)),
        ),
    )
    .await?;
    Ok(())
}
