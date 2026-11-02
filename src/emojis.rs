//! Application-emoji registry — the single place where UI icons are managed.
//!
//! [`emojis.toml`](../emojis.toml) maps a logical UI slot (`"skip"`, `"good"`, …)
//! to an emoji in the bot application's own emoji library, with a generic
//! Unicode fallback. Nothing in the UI layer hard-codes an emoji anymore.
//!
//! Resolution (once, in `ready`):
//!   1. explicit numeric `id` from the TOML,
//!   2. an existing application emoji whose `name` matches (read-only lookup),
//!   3. an optional one-time upload of the bundled PNG (`EMOJI_AUTO_UPLOAD=1`),
//!   4. the generic Unicode `fallback`.
//!
//! Every failure degrades to step 4, so a missing emoji, a rate limit or an
//! offline API can never stop the bot from starting. The registry is process
//! wide and immutable after startup: a `RwLock` gives cheap `Send + Sync`
//! reads without threading a handle through every `ui::` call site.

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use serde::Deserialize;
use serenity::{http::Http, model::prelude::*};
use std::{
    collections::HashMap,
    env, fs,
    sync::{
        OnceLock, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tracing::{debug, info, warn};

const DEFAULT_TOML: &str = include_str!("../emojis.toml");

/// Keys the UI depends on. A missing one is a configuration error, caught at
/// startup rather than turning into a silent "✦" on a button.
pub const REQUIRED: &[&str] = &[
    "back",
    "play",
    "pause",
    "skip",
    "stop",
    "loop",
    "volume_down",
    "volume_up",
    "shuffle",
    "queue",
    "refresh",
    "prev",
    "next",
    "page",
    "close",
    "cancel",
    "top",
    "search",
    "neutral",
    "good",
    "warn",
    "bad",
    "playing",
    "paused",
    "connected",
    "offline",
    "idle",
    "added",
    "youtube",
    "soundcloud",
    "direct",
    "link",
];

#[derive(Debug, Deserialize)]
#[serde(default)]
struct Meta {
    prefix: String,
    auto_upload: bool,
}

impl Default for Meta {
    fn default() -> Self {
        Self {
            prefix: "nx_".to_owned(),
            auto_upload: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Spec {
    name: String,
    fallback: String,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    id: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct File {
    meta: Meta,
    emojis: HashMap<String, Spec>,
}

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    id: Option<u64>,
    fallback: String,
    file: Option<String>,
}

/// An immutable snapshot of all resolved slots.
#[derive(Clone, Debug)]
pub struct EmojiMap {
    entries: HashMap<String, Entry>,
    auto_upload: bool,
}

impl EmojiMap {
    fn parse(raw: &str) -> Result<Self> {
        let file: File = toml::from_str(raw).context("emojis.toml is not valid TOML")?;

        for key in REQUIRED {
            if !file.emojis.contains_key(*key) {
                bail!("emojis.toml is missing the `{key}` slot");
            }
        }
        for (key, spec) in &file.emojis {
            if spec.fallback.trim().is_empty() {
                bail!("emojis.toml: `{key}` has an empty Unicode fallback");
            }
            // Keep every application emoji in one namespace (`nx_…`). Explicit
            // ID overrides may point anywhere, so they are exempt.
            if spec.id.is_none() && !spec.name.starts_with(&file.meta.prefix) {
                bail!(
                    "emojis.toml: `{key}` name {:?} must start with the configured prefix {:?}",
                    spec.name,
                    file.meta.prefix
                );
            }
            if !valid_emoji_name(&spec.name) {
                bail!(
                    "emojis.toml: `{key}` has an invalid Discord emoji name {:?} \
                     (2–32 alphanumeric/underscore characters)",
                    spec.name
                );
            }
        }

        let entries = file
            .emojis
            .into_iter()
            .map(|(key, spec)| {
                (
                    key,
                    Entry {
                        name: spec.name,
                        id: spec.id,
                        fallback: spec.fallback,
                        file: spec.file,
                    },
                )
            })
            .collect();

        Ok(Self {
            entries,
            auto_upload: file.meta.auto_upload,
        })
    }

    /// Load `EMOJIS_FILE` when set, otherwise the TOML compiled into the binary.
    pub fn load() -> Result<Self> {
        match env::var("EMOJIS_FILE")
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
        {
            Some(path) => {
                let raw = fs::read_to_string(&path)
                    .with_context(|| format!("cannot read EMOJIS_FILE={path}"))?;
                info!("loading emoji registry from {path}");
                Self::parse(&raw)
            }
            None => Self::parse(DEFAULT_TOML).context("built-in emojis.toml is invalid"),
        }
    }

    pub fn custom_count(&self) -> usize {
        self.entries.values().filter(|e| e.id.is_some()).count()
    }

    /// `ReactionType` for a button / select-menu component.
    pub fn reaction(&self, key: &str) -> ReactionType {
        match self.entries.get(key) {
            Some(entry) => match entry.id {
                Some(id) => ReactionType::Custom {
                    animated: false,
                    id: EmojiId::new(id),
                    name: Some(entry.name.clone()),
                },
                None => ReactionType::Unicode(entry.fallback.clone()),
            },
            None => {
                warn!("unknown emoji slot `{key}` — using neutral fallback");
                ReactionType::Unicode("✦".to_owned())
            }
        }
    }

    /// Inline emoji for embed text (`<:name:id>` or the Unicode fallback).
    pub fn mention(&self, key: &str) -> String {
        match self.entries.get(key) {
            Some(entry) => match entry.id {
                Some(id) => format!("<:{}:{}>", entry.name, id),
                None => entry.fallback.clone(),
            },
            None => {
                warn!("unknown emoji slot `{key}` — using neutral fallback");
                "✦".to_owned()
            }
        }
    }

    /// Resolve IDs against Discord's application emoji library. Never fails:
    /// API errors simply leave the Unicode fallbacks in place.
    pub async fn resolve(&self, http: &Http, force_upload: bool) -> Self {
        let mut entries = self.entries.clone();
        let remote: HashMap<String, u64> = match http.get_application_emojis().await {
            Ok(list) => list
                .into_iter()
                .map(|emoji| (emoji.name, emoji.id.get()))
                .collect(),
            Err(e) => {
                warn!(
                    "could not read the application emoji library ({e:?}) — using generic emojis"
                );
                HashMap::new()
            }
        };

        // Several slots may share one name (e.g. `play` / `playing`). Upload
        // each distinct name at most once per startup.
        let mut uploaded: HashMap<String, u64> = HashMap::new();
        let mut created = 0usize;
        let upload_enabled = force_upload || self.auto_upload;

        for entry in entries.values_mut() {
            if entry.id.is_some() {
                continue;
            }
            if let Some(id) = remote
                .get(&entry.name)
                .or_else(|| uploaded.get(&entry.name))
            {
                entry.id = Some(*id);
                continue;
            }
            if !upload_enabled {
                continue;
            }
            let Some(file) = entry.file.as_deref() else {
                continue;
            };
            let Some(bytes) = bundled_png(file) else {
                warn!("emojis.toml references {file}, which is not bundled — skipping upload");
                continue;
            };
            let payload = serde_json::json!({
                "name": entry.name,
                "image": format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                ),
            });
            match http.create_application_emoji(&payload).await {
                Ok(emoji) => {
                    let id = emoji.id.get();
                    entry.id = Some(id);
                    uploaded.insert(entry.name.clone(), id);
                    created += 1;
                }
                Err(e) => warn!(
                    "could not create application emoji {} ({e:?}) — using generic fallback",
                    entry.name
                ),
            }
        }

        let resolved = Self {
            entries,
            auto_upload: self.auto_upload,
        };
        info!(
            "emoji registry ready: {} custom slots, {} uploaded now, {} generic fallbacks",
            resolved.custom_count(),
            created,
            resolved.entries.len() - resolved.custom_count()
        );
        resolved
    }
}

/// Safety net used only if even the compiled TOML cannot be parsed.
fn emergency() -> EmojiMap {
    EmojiMap {
        entries: HashMap::from([(
            "neutral".to_owned(),
            Entry {
                name: "nx_neutral".to_owned(),
                id: None,
                fallback: "✦".to_owned(),
                file: None,
            },
        )]),
        auto_upload: false,
    }
}

fn valid_emoji_name(name: &str) -> bool {
    (2..=32).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// PNGs are compiled in, so uploads work from the minimal Docker image too.
fn bundled_png(file: &str) -> Option<&'static [u8]> {
    Some(match file {
        "nx_back.png" => include_bytes!("../assets/emojis/nx_back.png"),
        "nx_loop.png" => include_bytes!("../assets/emojis/nx_loop.png"),
        "nx_pause.png" => include_bytes!("../assets/emojis/nx_pause.png"),
        "nx_play.png" => include_bytes!("../assets/emojis/nx_play.png"),
        "nx_queue.png" => include_bytes!("../assets/emojis/nx_queue.png"),
        "nx_refresh.png" => include_bytes!("../assets/emojis/nx_refresh.png"),
        "nx_search.png" => include_bytes!("../assets/emojis/nx_search.png"),
        "nx_shuffle.png" => include_bytes!("../assets/emojis/nx_shuffle.png"),
        "nx_skip.png" => include_bytes!("../assets/emojis/nx_skip.png"),
        "nx_stop.png" => include_bytes!("../assets/emojis/nx_stop.png"),
        _ => return None,
    })
}

static REGISTRY: OnceLock<RwLock<EmojiMap>> = OnceLock::new();
static RESOLVED: AtomicBool = AtomicBool::new(false);

/// True once the Discord emoji library has been consulted (guards reconnects).
pub fn resolved() -> bool {
    RESOLVED.load(Ordering::Relaxed)
}

pub fn mark_resolved() {
    RESOLVED.store(true, Ordering::Relaxed);
}

fn registry() -> &'static RwLock<EmojiMap> {
    REGISTRY.get_or_init(|| {
        RwLock::new(EmojiMap::parse(DEFAULT_TOML).unwrap_or_else(|e| {
            warn!("built-in emoji registry is broken ({e:#}) — using emergency fallback");
            emergency()
        }))
    })
}

fn read() -> std::sync::RwLockReadGuard<'static, EmojiMap> {
    registry()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Install a new snapshot (called once with the loaded file, then again after
/// `ready` resolves the IDs). Safe to call on every gateway reconnect.
pub fn install(map: EmojiMap) {
    let mut guard = registry()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let changed = guard.custom_count() != map.custom_count();
    *guard = map;
    if changed {
        debug!("emoji registry updated");
    }
}

pub fn snapshot() -> EmojiMap {
    read().clone()
}

pub fn reaction(key: &str) -> ReactionType {
    read().reaction(key)
}

pub fn mention(key: &str) -> String {
    read().mention(key)
}

pub fn custom_count() -> usize {
    read().custom_count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_config_is_complete_and_valid() {
        let map = EmojiMap::parse(DEFAULT_TOML).expect("bundled emojis.toml must parse");
        for key in REQUIRED {
            assert!(map.entries.contains_key(*key), "missing slot {key}");
        }
        assert_eq!(
            map.custom_count(),
            0,
            "no IDs should be baked into the repo"
        );
    }

    #[test]
    fn falls_back_to_unicode_without_ids() {
        let map = EmojiMap::parse(DEFAULT_TOML).unwrap();
        assert_eq!(map.mention("skip"), "⏭");
        assert!(matches!(map.reaction("skip"), ReactionType::Unicode(_)));
    }

    #[test]
    fn explicit_id_produces_a_custom_mention() {
        let mut map = EmojiMap::parse(DEFAULT_TOML).unwrap();
        let entry = map.entries.get_mut("skip").unwrap();
        entry.id = Some(123_456_789);
        assert_eq!(map.mention("skip"), "<:nx_skip:123456789>");
        match map.reaction("skip") {
            ReactionType::Custom { id, name, .. } => {
                assert_eq!(id.get(), 123_456_789);
                assert_eq!(name.as_deref(), Some("nx_skip"));
            }
            _ => panic!("expected a custom reaction"),
        }
    }

    #[test]
    fn rejects_bad_emoji_names() {
        assert!(valid_emoji_name("nx_play"));
        assert!(!valid_emoji_name("x"));
        assert!(!valid_emoji_name("has spaces"));
        assert!(!valid_emoji_name("emoji-with-dashes"));
    }

    #[test]
    fn missing_slot_is_a_startup_error() {
        let raw = DEFAULT_TOML.replace("skip         =", "removed      =");
        assert!(EmojiMap::parse(&raw).is_err());
    }
}
