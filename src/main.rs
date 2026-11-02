//! Nexcord — a small, fast, keyless Discord music bot.
//!
//! serenity (gateway + REST) · songbird (voice) · yt-dlp (search/resolve)
//! No command framework, no database, no API keys, no runtime downloads until
//! a track actually starts playing.

mod commands;
mod config;
mod emojis;
mod interactions;
mod player;
mod search;
mod ui;
mod util;

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use player::AppState;
use serenity::{
    client::{Client, Context as SerenityContext, EventHandler},
    gateway::ActivityData,
    http::Http,
    model::prelude::*,
};
use songbird::{SerenityInit, Songbird};
use std::{sync::Arc, time::Duration};
use tracing::{info, warn};

struct Handler {
    app: Arc<AppState>,
}

/// Serenity's first HTTP request (`/users/@me`, `/gateway/bot`) has neither a
/// log line nor a timeout, so on a filtered network the process appears to
/// hang right after the `yt-dlp ready` message. Probe the same endpoint with a
/// short deadline so a bad token or a blocked network says so out loud.
async fn check_discord_api(client: &reqwest::Client, token: &str) -> Result<()> {
    let auth = if token.starts_with("Bot ") {
        token.to_owned()
    } else {
        format!("Bot {token}")
    };
    let response = client
        .get("https://discord.com/api/v10/users/@me")
        .header(reqwest::header::AUTHORIZATION, auth)
        .timeout(Duration::from_secs(10))
        .send()
        .await;

    match response {
        Ok(resp) => match resp.status().as_u16() {
            200 => {
                // reqwest is built without the `json` feature; parse the tiny
                // response with the serde_json we already depend on.
                let name = resp
                    .text()
                    .await
                    .ok()
                    .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
                    .and_then(|v| v["username"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| "bot".to_owned());
                info!("Discord API reachable — token accepted as @{name}");
            }
            401 => bail!(
                "Discord rejected DISCORD_TOKEN (401 Unauthorized). Reset it in the \
                 Developer Portal → Bot → Reset Token and update .env"
            ),
            403 => {
                bail!("Discord returned 403 — DISCORD_TOKEN does not belong to a bot application")
            }
            429 => warn!("Discord rate-limited the startup check (429); this is temporary"),
            code => warn!("Discord API returned HTTP {code} during the startup check"),
        },
        Err(e) => warn!(
            "cannot reach discord.com ({e:#}) — check internet, DNS, firewall, proxy or VPN; \
             the bot cannot come online until this works"
        ),
    }
    Ok(())
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: SerenityContext, ready: Ready) {
        info!("gateway READY for {} — finishing setup", ready.user.name);
        // Needed for interaction endpoints on the standalone Http client too.
        self.app.http.set_application_id(ready.application.id);

        // Resolve the application emoji library once. Reconnects reuse the
        // resolved IDs instead of hitting the API again.
        if !emojis::resolved() {
            let base = emojis::snapshot();
            let resolved = base
                .resolve(&self.app.http, self.app.config.emoji_auto_upload)
                .await;
            emojis::install(resolved);
            emojis::mark_resolved();
        }
        ctx.set_activity(Some(ActivityData::listening("/play  ·  /search")));

        let commands = commands::all();
        let total = commands.len();
        let result = match self.app.config.guild_id {
            Some(id) => GuildId::new(id)
                .set_commands(&ctx.http, commands)
                .await
                .map(|_| ()),
            None => Command::set_global_commands(&ctx.http, commands)
                .await
                .map(|_| ()),
        };
        match result {
            Ok(()) => info!(
                "registered {total} slash commands ({})",
                if self.app.config.guild_id.is_some() {
                    "guild scope"
                } else {
                    "global scope"
                }
            ),
            Err(e) => warn!("slash command registration failed: {e:?}"),
        }
        info!("{} is online", ready.user.name);
    }

    async fn interaction_create(&self, ctx: SerenityContext, interaction: Interaction) {
        // Every interaction is independent: never block the gateway dispatch.
        let app = self.app.clone();
        tokio::spawn(async move {
            interactions::dispatch(ctx, interaction, app).await;
        });
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("nexcord=info,songbird=warn,serenity=warn")
            }),
        )
        .init();

    let config = config::load()?;
    // Fail fast on a broken emoji file rather than silently showing ✦ on
    // every button. The compiled default is used when EMOJIS_FILE is unset.
    emojis::install(emojis::EmojiMap::load().context("invalid emoji configuration")?);
    let http = Arc::new(Http::new(&config.token));
    let voice = Songbird::serenity_from_config(config.voice_config());
    let app = Arc::new(AppState::new(config.clone(), http, voice.clone()));

    match app.searcher.check_available().await {
        Ok(version) => info!("yt-dlp {version} ready — keyless search enabled"),
        Err(e) => warn!("{e:#} — /play and /search need yt-dlp on PATH"),
    }
    // Discord rejects non-DAVE voice connections with close code 4017. This
    // build is pinned to songbird 0.6.1, whose Identify advertises
    // `max_dave_protocol_version` via the pure-Rust `davey` crate.
    info!("voice engine ready — songbird 0.6.1 with DAVE (E2EE) support");

    // Expire search caches / pickers and leave silent voice channels.
    {
        let app = app.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(30));
            ticker.tick().await; // consume the immediate first tick
            loop {
                ticker.tick().await;
                app.housekeeping().await;
            }
        });
    }

    // Only what's needed: guild list + voice states. No message content, no
    // presence intent, no privileged intents, minimal cache pressure.
    info!("connecting to Discord…");
    check_discord_api(&app.client, &config.token).await?;

    let intents = GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES;
    let build = Client::builder(config.token.clone(), intents)
        .event_handler(Handler { app: app.clone() })
        .register_songbird_with(voice);
    let mut client = tokio::time::timeout(Duration::from_secs(20), build)
        .await
        .context(
            "timed out after 20 s contacting Discord — see the connectivity warning above \
             (discord.com must be reachable)",
        )?
        .context("failed to build the Discord client")?;

    info!("opening the Discord gateway…");
    client
        .start()
        .await
        .context("Discord gateway exited before the bot came online")?;
    Ok(())
}
