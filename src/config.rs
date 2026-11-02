//! Environment configuration. Everything has a sane default except the bot token.

use anyhow::{bail, Context, Result};
use std::env;
use tracing::warn;

#[derive(Clone, Debug)]
pub struct Config {
    pub token: String,
    /// `Some` => register commands in a single guild (instant, great for dev).
    pub guild_id: Option<u64>,
    pub yt_dlp: String,
    pub default_volume: f32,
    pub max_queue: usize,
    pub search_results: usize,
    pub search_cache_ttl: std::time::Duration,
    pub idle_timeout: std::time::Duration,
    pub accent: u32,
    pub opus_bitrate: i32,
    pub asset_base_url: Option<String>,
    /// Upload missing application emojis once at startup (off by default).
    pub emoji_auto_upload: bool,
}

fn var(key: &str, default: &str) -> String {
    env::var(key)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn num<T>(key: &str, default: T) -> T
where
    T: std::str::FromStr + std::fmt::Display + Copy,
{
    let raw = var(key, "");
    if raw.is_empty() {
        return default;
    }
    match raw.parse::<T>() {
        Ok(value) => value,
        // A typo used to silently pick the default. Say so.
        Err(_) => {
            warn!("{key}={raw:?} is not a number — using {default}");
            default
        }
    }
}

fn colour(key: &str, default: u32) -> u32 {
    let raw = var(key, "");
    if raw.is_empty() {
        return default;
    }
    let cleaned = raw.trim_start_matches('#');
    let value = if cleaned.starts_with("0x") || cleaned.starts_with("0X") {
        u32::from_str_radix(&cleaned[2..], 16)
    } else if cleaned.chars().all(|c| c.is_ascii_hexdigit()) && cleaned.len() == 6 {
        u32::from_str_radix(cleaned, 16)
    } else {
        cleaned.parse::<u32>()
    };
    value.unwrap_or(default) & 0x00ff_ffff
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let token = var("DISCORD_TOKEN", "");
        if token.is_empty() {
            bail!("DISCORD_TOKEN is missing — copy .env.example to .env and fill it in");
        }
        if !token.starts_with("Bot ") && token.split('.').count() < 3 {
            bail!("DISCORD_TOKEN does not look like a Discord bot token");
        }

        let default_volume = num::<u32>("DEFAULT_VOLUME", 100).clamp(0, 200) as f32 / 100.0;
        let search_results = num::<usize>("SEARCH_RESULTS", 10).clamp(3, 25);

        Ok(Self {
            token,
            guild_id: var("GUILD_ID", "").parse::<u64>().ok().filter(|id| *id > 0),
            yt_dlp: var("YT_DLP", "yt-dlp"),
            default_volume,
            max_queue: num::<usize>("MAX_QUEUE", 200).clamp(1, 1000),
            search_results,
            search_cache_ttl: std::time::Duration::from_secs(
                num::<u64>("SEARCH_CACHE_TTL", 600).clamp(30, 86_400),
            ),
            idle_timeout: std::time::Duration::from_secs(num::<u64>("IDLE_TIMEOUT", 300)),
            accent: colour("ACCENT", 0x6c5ce7),
            opus_bitrate: num::<i32>("OPUS_BITRATE", 160_000).clamp(32_000, 384_000),
            asset_base_url: env::var("ASSET_BASE_URL")
                .ok()
                .map(|v| v.trim().trim_end_matches('/').to_owned())
                .filter(|v| !v.is_empty()),
            emoji_auto_upload: matches!(
                var("EMOJI_AUTO_UPLOAD", "").to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ),
        })
    }

    /// Songbird's voice config: stereo mixing, soft clipping (no distortion at
    /// high volume), and an explicit high Opus bitrate.
    pub fn voice_config(&self) -> songbird::Config {
        use songbird::driver::MixMode;
        songbird::Config::default()
            .mix_mode(MixMode::Stereo)
            .use_softclip(true)
            .preallocated_tracks(1)
    }
}

/// Helper used by `main` to give a clear error when a token is malformed.
pub fn load() -> Result<Config> {
    dotenvy::dotenv().ok();
    Config::from_env().context("failed to load configuration")
}

#[cfg(test)]
mod tests {
    use super::colour;

    #[test]
    fn parses_accent_colours() {
        assert_eq!(colour("ACCENT", 0x6c5ce7), 0x6c5ce7);
        assert_eq!(colour("", 0x6c5ce7), 0x6c5ce7);
        // `0x` / `#` / plain-hex / decimal are all accepted.
        std::env::set_var("NX_TEST_ACCENT", "0x7B6CF6");
        assert_eq!(colour("NX_TEST_ACCENT", 0), 0x7b6cf6);
        std::env::set_var("NX_TEST_ACCENT", "#35D6E8");
        assert_eq!(colour("NX_TEST_ACCENT", 0), 0x35d6e8);
        std::env::set_var("NX_TEST_ACCENT", "not-a-colour");
        assert_eq!(colour("NX_TEST_ACCENT", 42), 42);
        std::env::remove_var("NX_TEST_ACCENT");
    }
}
