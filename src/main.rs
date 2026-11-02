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

use anyhow::{Context as _, Result};
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

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: SerenityContext, ready: Ready) {
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
    let intents = GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES;
    let mut client = Client::builder(config.token.clone(), intents)
        .event_handler(Handler { app: app.clone() })
        .register_songbird_with(voice)
        .await
        .context("failed to build the Discord client")?;

    info!("starting Nexcord…");
    client.start().await?;
    Ok(())
}
