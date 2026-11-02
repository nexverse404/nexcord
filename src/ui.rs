//! All embed + component construction lives here, so the design stays in one
//! place and the rest of the bot never deals with Discord's JSON shapes.
//!
//! Design language (benchmarked against professional music bots — Chip, Hade,
//! MusikBot's live player, PlexBot's classic player):
//!
//! * one clear hierarchy: header → track title (a link) → one metadata line →
//!   one progress line → one status line → a small footer;
//! * **no** blockquotes, no empty `>` spacer lines, no walls of inline-code
//!   backticks, no spaced-out `N E X C O R D` headers;
//! * at most **one** image while a track is playing (its album art as a small
//!   right-hand thumbnail), never a full-width banner *and* a thumbnail;
//! * plain short labels, bold only for values, two compact button rows.
//!
//! Generated artwork is compiled in (~100 KB). Every attachment is referenced
//! by the embed, because Discord displays any *unreferenced* attachment as a
//! separate image above the embed. `refresh_panel` also replaces the attachment
//! set on every edit so a stale file can never linger.

use crate::{
    emojis,
    player::Snapshot,
    search::{Provider, Track},
    util,
};
use serenity::{builder::*, model::prelude::*};
use std::time::Duration;

const MARK_FILE: &str = "nexcord-mark.jpg";
const WAVE_FILE: &str = "nexcord-wave.jpg";
const COVER_FILE: &str = "nexcord-cover.jpg";

const MARK: &[u8] = include_bytes!("../assets/nexcord-mark.jpg");
const WAVE: &[u8] = include_bytes!("../assets/nexcord-wave.jpg");
const COVER: &[u8] = include_bytes!("../assets/nexcord-cover.jpg");

pub const ACCENT_FALLBACK: u32 = 0x6c5ce7;
const IDLE_COLOUR: u32 = 0x2b2f3a;
const PAUSED_COLOUR: u32 = 0xf5a524;

#[derive(Clone, Debug)]
pub struct Assets {
    base_url: Option<String>,
}

impl Assets {
    pub fn new(base_url: Option<String>) -> Self {
        Self { base_url }
    }

    pub fn needs_upload(&self) -> bool {
        self.base_url.is_none()
    }

    pub fn get(&self, file: &str) -> String {
        match &self.base_url {
            Some(base) => format!("{base}/{file}"),
            None => format!("attachment://{file}"),
        }
    }

    /// Attachments for a panel in a given state. `idle` cards show the generated
    /// wave banner and cover art; a playing card shows only the small brand icon
    /// (author/footer) plus the track's own remote album art. Every returned
    /// file is referenced by `panel_embed`, so none is rendered standalone.
    pub fn attachments(&self, idle: bool) -> Vec<CreateAttachment> {
        if !self.needs_upload() {
            return Vec::new();
        }
        let mut out = vec![CreateAttachment::bytes(MARK.to_vec(), MARK_FILE)];
        if idle {
            out.push(CreateAttachment::bytes(WAVE.to_vec(), WAVE_FILE));
            out.push(CreateAttachment::bytes(COVER.to_vec(), COVER_FILE));
        }
        out
    }
}

pub enum Tone {
    Neutral,
    Good,
    Warn,
    Bad,
}

impl Tone {
    fn colour(&self) -> Colour {
        match self {
            Self::Neutral => Colour::new(ACCENT_FALLBACK),
            Self::Good => Colour::new(0x3ddc97),
            Self::Warn => Colour::new(0xf5a524),
            Self::Bad => Colour::new(0xf25f5c),
        }
    }

    fn key(&self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Good => "good",
            Self::Warn => "warn",
            Self::Bad => "bad",
        }
    }
}

fn header(label: &str, assets: Option<&Assets>) -> CreateEmbedAuthor {
    let mut author = CreateEmbedAuthor::new(format!("NEXCORD  ·  {label}"));
    if let Some(assets) = assets {
        author = author.icon_url(assets.get(MARK_FILE));
    }
    author
}

pub fn notice(title: &str, description: &str, tone: Tone) -> CreateEmbed {
    CreateEmbed::new()
        .author(header("NOTICE", None))
        .title(format!("{}  {}", emojis::mention(tone.key()), title))
        .description(description)
        .color(tone.colour())
        .footer(CreateEmbedFooter::new("NEXCORD"))
}

fn button(id: String, slot: &str, label: &str, style: ButtonStyle, disabled: bool) -> CreateButton {
    // Discord caps custom_id at 100 bytes; keep every component valid even
    // if a session id grows unexpectedly.
    CreateButton::new(util::clamp_id(&id))
        .style(style)
        .emoji(emojis::reaction(slot))
        .label(label)
        .disabled(disabled)
}

fn state_line(snap: &Snapshot) -> String {
    let status = if snap.paused { "Paused" } else { "Playing" };
    let connection = if snap.connected {
        "Connected"
    } else {
        "Offline"
    };
    format!(
        "{} **{}**  ·  {} {}",
        emojis::mention(if snap.paused { "paused" } else { "playing" }),
        status,
        emojis::mention(if snap.connected {
            "connected"
        } else {
            "offline"
        }),
        connection,
    )
}

fn progress_line(snap: &Snapshot, track: &Track) -> String {
    let total = track
        .duration
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(Duration::from_secs_f64);
    format!(
        "{}  {}  {}",
        util::fmt_duration(snap.position),
        util::progress_bar(snap.position, total, 16),
        util::fmt_duration(total.unwrap_or_default()),
    )
}

/// The persistent music control panel.
pub fn panel_embed(snap: &Snapshot, assets: &Assets, accent: u32) -> CreateEmbed {
    let idle = snap.current.is_none();
    let colour = if idle {
        Colour::new(IDLE_COLOUR)
    } else if snap.paused {
        Colour::new(PAUSED_COLOUR)
    } else {
        Colour::new(accent)
    };

    let mut embed = CreateEmbed::new()
        .author(header("NOW PLAYING", Some(assets)))
        .color(colour);

    if let Some(track) = snap.current.as_ref() {
        let artist = if track.author.is_empty() {
            "Unknown artist"
        } else {
            &track.author
        };
        let description = format!(
            "{}\n\
             **{}**  ·  {}  ·  {} {}\n\
             {}\n\
             Volume **{}%**  ·  Loop **{}**  ·  **{}** in queue",
            state_line(snap),
            util::clean_md(&util::truncate(artist, 64)),
            util::fmt_secs(track.duration),
            emojis::mention(track.source.emoji_key()),
            track.source.label(),
            progress_line(snap, track),
            (snap.volume * 100.0).round() as i32,
            snap.loop_mode.label(),
            snap.queue_len,
        );
        embed = embed
            .title(util::truncate(&track.title, 100))
            .url(track.url.clone())
            .description(description);

        if let Some(art) = &track.thumbnail {
            // Remote album art only: no attachment is needed for it.
            embed = embed.thumbnail(art.clone());
        }

        if !snap.queue.is_empty() {
            let mut lines = String::with_capacity(160);
            for (i, next) in snap.queue.iter().take(3).enumerate() {
                lines.push_str(&format!(
                    "`{}.` {} — {}\n",
                    i + 1,
                    util::clean_md(&util::truncate(&next.title, 52)),
                    util::fmt_secs(next.duration),
                ));
            }
            if snap.queue_len > 3 {
                lines.push_str(&format!("… and **{}** more", snap.queue_len - 3));
            }
            embed = embed.field("UP NEXT", lines, false);
        }
        embed = embed.footer(
            CreateEmbedFooter::new(format!(
                "requested by {}  ·  NEXCORD",
                util::truncate(&track.requester, 40)
            ))
            .icon_url(assets.get(MARK_FILE)),
        );
    } else {
        embed = embed
            .title("Nothing playing")
            .description(
                "**Ready when you are.**\n\
                 `/play` a song, a link or a playlist — or `/search` to pick an exact match.\n\
                 YouTube, SoundCloud, Bandcamp and 1000+ sites. No API key needed.",
            )
            .image(assets.get(WAVE_FILE))
            .thumbnail(assets.get(COVER_FILE))
            .footer(
                CreateEmbedFooter::new("NEXCORD  ·  fast · clean · keyless")
                    .icon_url(assets.get(MARK_FILE)),
            );
    }

    embed
}

pub fn panel_components(snap: &Snapshot) -> Vec<CreateActionRow> {
    let playing = snap.current.is_some();
    let play_slot = if snap.paused { "play" } else { "pause" };
    let play_label = if snap.paused { "Resume" } else { "Pause" };
    let loop_label = format!("Loop · {}", snap.loop_mode.label());

    vec![
        CreateActionRow::Buttons(vec![
            button(
                "nx:back".into(),
                "back",
                "−15s",
                ButtonStyle::Secondary,
                !playing,
            ),
            button(
                "nx:playpause".into(),
                play_slot,
                play_label,
                ButtonStyle::Primary,
                !playing,
            ),
            button(
                "nx:skip".into(),
                "skip",
                "Skip",
                ButtonStyle::Secondary,
                !playing,
            ),
            button(
                "nx:stop".into(),
                "stop",
                "Stop",
                ButtonStyle::Danger,
                !playing,
            ),
            button(
                "nx:loop".into(),
                "loop",
                &loop_label,
                ButtonStyle::Secondary,
                false,
            ),
        ]),
        CreateActionRow::Buttons(vec![
            button(
                "nx:voldown".into(),
                "volume_down",
                "Vol −",
                ButtonStyle::Secondary,
                false,
            ),
            button(
                "nx:volup".into(),
                "volume_up",
                "Vol +",
                ButtonStyle::Secondary,
                false,
            ),
            button(
                "nx:shuffle".into(),
                "shuffle",
                "Shuffle",
                ButtonStyle::Secondary,
                snap.queue_len < 2,
            ),
            button(
                "nx:queue".into(),
                "queue",
                "Queue",
                ButtonStyle::Secondary,
                false,
            ),
            button(
                "nx:refresh".into(),
                "refresh",
                "Refresh",
                ButtonStyle::Secondary,
                false,
            ),
        ]),
    ]
}

pub const QUEUE_PER_PAGE: usize = 10;

pub fn queue_pages(len: usize) -> usize {
    if len == 0 {
        1
    } else {
        len.div_ceil(QUEUE_PER_PAGE)
    }
}

pub fn queue_embed(snap: &Snapshot, page: usize, pages: usize) -> CreateEmbed {
    let mut embed = CreateEmbed::new()
        .author(header("QUEUE", None))
        .title(format!(
            "Up next  ·  {} track{}",
            snap.queue_len,
            if snap.queue_len == 1 { "" } else { "s" }
        ))
        .color(Colour::new(ACCENT_FALLBACK));

    if let Some(current) = snap.current.as_ref() {
        embed = embed.field(
            "NOW PLAYING",
            format!(
                "**{}**\n{}  ·  {}",
                util::clean_md(&util::truncate(&current.title, 90)),
                util::clean_md(&util::truncate(&current.author, 50)),
                util::fmt_secs(current.duration)
            ),
            false,
        );
    }

    if snap.queue.is_empty() {
        embed = embed
            .description("The queue is empty. `/play` something, or `/search` for an exact match.");
    } else {
        let start = (page * QUEUE_PER_PAGE).min(snap.queue.len());
        let end = (start + QUEUE_PER_PAGE).min(snap.queue.len());
        let mut lines = String::new();
        for (i, t) in snap.queue[start..end].iter().enumerate() {
            lines.push_str(&format!(
                "`{:>2}.`  **{}**\n      {}  ·  {}  ·  {}\n",
                start + i + 1,
                util::clean_md(&util::truncate(&t.title, 68)),
                util::clean_md(&util::truncate(&t.author, 42)),
                util::fmt_secs(t.duration),
                t.source.label()
            ));
        }
        embed = embed.description(lines);
    }

    let total_secs: f64 = snap.queue.iter().filter_map(|t| t.duration).sum();
    embed.footer(CreateEmbedFooter::new(format!(
        "page {}/{}  ·  {} queued  ·  {} remaining  ·  loop {}",
        page + 1,
        pages,
        snap.queue_len,
        util::fmt_duration(Duration::from_secs(total_secs.max(0.0) as u64)),
        snap.loop_mode.label()
    )))
}

pub fn queue_components(page: usize, pages: usize) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        button(
            "nx:qprev".into(),
            "prev",
            "Prev",
            ButtonStyle::Secondary,
            page == 0,
        ),
        button(
            "nx:qpage".into(),
            "page",
            &format!("{}/{}", page + 1, pages),
            ButtonStyle::Secondary,
            true,
        ),
        button(
            "nx:qnext".into(),
            "next",
            "Next",
            ButtonStyle::Secondary,
            page + 1 >= pages,
        ),
        button(
            "nx:qclose".into(),
            "close",
            "Close",
            ButtonStyle::Secondary,
            false,
        ),
    ])]
}

pub fn search_embed(query: &str, tracks: &[Track], provider: Provider) -> CreateEmbed {
    let mut lines = String::new();
    for (i, t) in tracks.iter().take(10).enumerate() {
        lines.push_str(&format!(
            "`{:>2}.`  [{}]({})\n      {}  ·  {}\n",
            i + 1,
            util::clean_md(&util::truncate(&t.title, 74)),
            t.url,
            util::clean_md(&util::truncate(&t.subtitle(), 70)),
            util::fmt_secs(t.duration),
        ));
    }
    CreateEmbed::new()
        .author(header("SEARCH", None))
        .title(format!(
            "Results for “{}”",
            util::truncate(&util::clean_md(query), 70)
        ))
        .description(util::truncate(&lines, 3900))
        .color(Colour::new(ACCENT_FALLBACK))
        .footer(CreateEmbedFooter::new(format!(
            "{}  ·  {} matches  ·  pick one below  ·  expires in 10 min",
            provider.label(),
            tracks.len()
        )))
}

pub fn search_components(session: &str, tracks: &[Track]) -> Vec<CreateActionRow> {
    if tracks.is_empty() {
        // Discord rejects a StringSelect with an empty option list.
        return vec![CreateActionRow::Buttons(vec![button(
            "nx:cancel".into(),
            "cancel",
            "Cancel",
            ButtonStyle::Secondary,
            false,
        )])];
    }
    let options: Vec<CreateSelectMenuOption> = tracks
        .iter()
        .take(25)
        .enumerate()
        .map(|(i, t)| {
            CreateSelectMenuOption::new(
                format!("{}. {}", i + 1, util::truncate(&t.title, 88)),
                util::clamp_id(&format!("{session}:{i}")),
            )
            .description(util::truncate(&t.subtitle(), 90))
        })
        .collect();

    vec![
        CreateActionRow::SelectMenu(
            CreateSelectMenu::new("nx:pick", CreateSelectMenuKind::String { options })
                .placeholder("Pick the exact track…")
                .min_values(1)
                .max_values(1),
        ),
        CreateActionRow::Buttons(vec![
            button(
                format!("nx:top:{session}"),
                "top",
                "Play best match",
                ButtonStyle::Success,
                false,
            ),
            button(
                "nx:cancel".into(),
                "cancel",
                "Cancel",
                ButtonStyle::Secondary,
                false,
            ),
        ]),
    ]
}

pub fn added_embed(track: &Track, position: usize) -> CreateEmbed {
    let state = if position == 0 {
        "Playing now".to_owned()
    } else {
        format!("Added to queue · position **{position}**")
    };
    CreateEmbed::new()
        .author(header("QUEUED", None))
        .title(util::truncate(&track.title, 100))
        .url(track.url.clone())
        .description(format!(
            "{}  **{}**\n{}  ·  {}  ·  {}\n\nrequested by **{}**",
            emojis::mention(if position == 0 { "playing" } else { "added" }),
            state,
            util::clean_md(&util::truncate(&track.author, 64)),
            util::fmt_secs(track.duration),
            track.source.label(),
            util::clean_md(&util::truncate(&track.requester, 40))
        ))
        .color(Colour::new(0x3ddc97))
        .footer(CreateEmbedFooter::new("NEXCORD"))
}

/// Compact now-playing card without `attachment://` references, so it can be
/// sent as a plain ephemeral interaction response.
pub fn now_embed(snap: &Snapshot) -> CreateEmbed {
    match snap.current.as_ref() {
        Some(track) => {
            let artist = if track.author.is_empty() {
                "Unknown artist"
            } else {
                &track.author
            };
            let mut embed = CreateEmbed::new()
                .author(header("NOW PLAYING", None))
                .title(util::truncate(&track.title, 100))
                .url(track.url.clone())
                .description(format!(
                    "{}\n\
                     **{}**  ·  {}  ·  {} {}\n\
                     {}\n\
                     Volume **{}%**  ·  Loop **{}**  ·  **{}** in queue",
                    state_line(snap),
                    util::clean_md(&util::truncate(artist, 64)),
                    util::fmt_secs(track.duration),
                    emojis::mention(track.source.emoji_key()),
                    track.source.label(),
                    progress_line(snap, track),
                    (snap.volume * 100.0).round() as i32,
                    snap.loop_mode.label(),
                    snap.queue_len,
                ))
                .color(Colour::new(ACCENT_FALLBACK))
                .footer(CreateEmbedFooter::new(format!(
                    "requested by {}  ·  NEXCORD",
                    util::truncate(&track.requester, 48)
                )));
            if let Some(art) = &track.thumbnail {
                embed = embed.thumbnail(art.clone());
            }
            embed
        }
        None => notice(
            "Nothing playing",
            "Use `/play` with a song name or any link — or `/search` to pick an exact match.",
            Tone::Neutral,
        ),
    }
}

pub fn help_embed() -> CreateEmbed {
    CreateEmbed::new()
        .author(header("HELP", None))
        .title("Everything, in one place")
        .description("A keyless music bot. Type a song name, paste a link, or pick an exact result.")
        .color(Colour::new(ACCENT_FALLBACK))
        .fields(vec![
            (
                "PLAYBACK",
                "`/play` `/search` `/pause` `/skip` `/stop` `/seek`\n`/join` `/leave` `/panel`",
                false,
            ),
            (
                "QUEUE & SOUND",
                "`/queue` `/shuffle` `/loop` `/remove` `/volume`\nControls also live on the music panel buttons.",
                false,
            ),
            (
                "SEARCH, NO KEYS",
                "Searches YouTube **and** SoundCloud in parallel. Paste links from Bandcamp, archive.org, direct audio files, playlists — anything yt-dlp supports. Use `pick:true` or `/search` to choose the exact match.",
                false,
            ),
            ("INFO", "`/now` `/stats` `/help`", false),
        ])
        .footer(CreateEmbedFooter::new("NEXCORD  ·  built in Rust  ·  48 kHz stereo Opus"))
}

pub fn stats_embed(fields: Vec<(String, String, bool)>) -> CreateEmbed {
    CreateEmbed::new()
        .author(header("STATUS", None))
        .title("Runtime")
        .color(Colour::new(ACCENT_FALLBACK))
        .fields(fields)
        .footer(CreateEmbedFooter::new("NEXCORD  ·  lightweight by design"))
}
