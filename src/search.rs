//! Keyless, accurate music search powered exclusively by `yt-dlp`.
//!
//! Design goals
//! ------------
//! * **No API keys.** `yt-dlp` talks to YouTube, SoundCloud, Bandcamp, archive.org
//!   and 1000+ other extractors directly. There is no quota and no account.
//! * **Accurate for obscure music.** We search two catalogues concurrently
//!   (`ytsearch` + `scsearch`), score candidates against the literal query,
//!   de-duplicate, and — crucially — let the user pick the exact match from a
//!   select menu. Direct URLs of *any* yt-dlp supported site also work.
//! * **Fast.** `--flat-playlist` means one process, one request, no per-video
//!   metadata fetches. Results are cached in memory and provider calls run in
//!   parallel. Nothing is downloaded until a track actually plays.

use crate::util::{normalize, tokens};
use anyhow::{Context as _, Result, bail};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::{process::Command, time::timeout};
use tracing::{debug, warn};

/// Hard ceiling for one extractor. Discord gives an interaction ~3 s before
/// deferral and our dispatcher 30 s overall; a stalled SoundCloud lookup must
/// never consume that whole budget.
const PROVIDER_DEADLINE: Duration = Duration::from_secs(9);

/// Baseline flags shared by search, link resolution and songbird playback.
/// `--socket-timeout`/`--retries` stop a hung extractor; the YouTube player
/// client list avoids the 2026 "Sign in to confirm you're not a bot" gate on
/// most IPs. `YT_DLP_ARGS` (cookies, PO-token providers, …) is appended after.
pub fn default_ytdlp_args() -> Vec<String> {
    vec![
        "--socket-timeout".into(),
        "6".into(),
        "--retries".into(),
        "1".into(),
        "--extractor-args".into(),
        "youtube:player_client=tv,web_safari".into(),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    YouTube,
    SoundCloud,
    Direct,
    Other(String),
}

impl Source {
    pub fn label(&self) -> &str {
        match self {
            Self::YouTube => "YouTube",
            Self::SoundCloud => "SoundCloud",
            Self::Direct => "Audio file",
            Self::Other(s) => s.as_str(),
        }
    }

    /// Key into `emojis.toml`; presentation stays in the UI layer.
    pub fn emoji_key(&self) -> &'static str {
        match self {
            Self::YouTube => "youtube",
            Self::SoundCloud => "soundcloud",
            Self::Direct => "direct",
            Self::Other(_) => "link",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Track {
    pub title: String,
    pub author: String,
    /// The page/stream URL handed to songbird's `YoutubeDl` input.
    pub url: String,
    pub thumbnail: Option<String>,
    pub duration: Option<f64>,
    pub source: Source,
    pub requester: String,
}

impl Track {
    /// One-line description used in embeds and select menus.
    pub fn subtitle(&self) -> String {
        let artist = if self.author.is_empty() {
            "Unknown artist"
        } else {
            &self.author
        };
        format!(
            "{} · {} · {}",
            crate::util::truncate(artist, 42),
            crate::util::fmt_secs(self.duration),
            self.source.label()
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Auto,
    YouTube,
    SoundCloud,
}

impl Provider {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "youtube" | "yt" => Self::YouTube,
            "soundcloud" | "sc" => Self::SoundCloud,
            _ => Self::Auto,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Auto => "YouTube + SoundCloud",
            Self::YouTube => "YouTube",
            Self::SoundCloud => "SoundCloud",
        }
    }
}

struct Cached {
    at: Instant,
    tracks: Vec<Track>,
}

pub struct Searcher {
    program: String,
    /// Extra flags appended to every yt-dlp invocation (cookies, PO tokens…).
    extra: Vec<String>,
    cache: Mutex<HashMap<String, Cached>>,
    limit: usize,
    ttl: Duration,
}

impl Searcher {
    pub fn new(program: String, user_args: Vec<String>, limit: usize, ttl: Duration) -> Self {
        let mut extra = default_ytdlp_args();
        extra.extend(user_args);
        Self {
            program,
            extra,
            cache: Mutex::new(HashMap::new()),
            limit,
            ttl,
        }
    }

    pub fn cache_len(&self) -> usize {
        self.cache.lock().map(|c| c.len()).unwrap_or(0)
    }

    /// Prefix-aware, cache-only suggestions for autocomplete.
    ///
    /// The old exact-key lookup meant every keystroke (`phir`, `phire`,
    /// `phiriye dao`) missed and started a new 20 s search, so the dropdown
    /// stayed empty. Now a cached result for a longer/shorter version of the
    /// same query is reused, and the closest-length match wins.
    pub fn suggestions(&self, query: &str) -> Vec<Track> {
        let q = normalize(query);
        if q.len() < 2 {
            return Vec::new();
        }
        let Ok(cache) = self.cache.lock() else {
            return Vec::new();
        };
        let mut best: Option<(&str, &Vec<Track>)> = None;
        for (key, hit) in cache.iter() {
            if hit.at.elapsed() >= self.ttl {
                continue;
            }
            let Some((_, cached_q)) = key.split_once('|') else {
                continue;
            };
            if !(cached_q.starts_with(&q) || q.starts_with(cached_q)) {
                continue;
            }
            let better = match best {
                None => true,
                Some((prev, _)) => cached_q.len().abs_diff(q.len()) < prev.len().abs_diff(q.len()),
            };
            if better {
                best = Some((cached_q, &hit.tracks));
            }
        }
        let Some((_, tracks)) = best else {
            return Vec::new();
        };
        let mut seen: HashSet<String> = HashSet::new();
        tracks
            .iter()
            .filter(|t| seen.insert(t.url.clone()))
            .take(10)
            .cloned()
            .collect()
    }

    /// Bounded YouTube-only search used to warm autocomplete in the background.
    /// Results land in the cache under the typed query so the next keystroke can
    /// show real suggestions instead of an empty dropdown.
    pub async fn quick_search(&self, query: &str) {
        let _ = timeout(
            Duration::from_secs(8),
            self.search(query, Provider::YouTube),
        )
        .await;
    }

    pub fn purge(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.retain(|_, v| v.at.elapsed() < self.ttl);
        }
    }

    /// Fail early (with a helpful message) if yt-dlp isn't installed.
    pub async fn check_available(&self) -> Result<String> {
        let out = timeout(
            // First-run Python imports can be slow; give yt-dlp room to answer.
            Duration::from_secs(15),
            Command::new(&self.program)
                .arg("--version")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("yt-dlp --version timed out")?
        .with_context(|| {
            format!(
                "cannot run '{}' — install yt-dlp (pip install -U yt-dlp) or set YT_DLP",
                self.program
            )
        })?;
        if !out.status.success() {
            bail!("'{}' is not a working yt-dlp executable", self.program);
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    /// Search, rank and de-duplicate candidates. Cached for `ttl`.
    pub async fn search(&self, query: &str, provider: Provider) -> Result<Vec<Track>> {
        let query = query.trim();
        if query.is_empty() {
            bail!("give me something to search for");
        }
        let key = format!("{provider:?}|{}", normalize(query));
        if let Ok(cache) = self.cache.lock() {
            if let Some(hit) = cache.get(&key) {
                if hit.at.elapsed() < self.ttl {
                    debug!("search cache hit for {query:?}");
                    return Ok(hit.tracks.clone());
                }
            }
        }

        let mut all: Vec<Track> = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        // Each catalogue gets its own deadline. A timeout is treated as "this
        // provider has nothing right now", not as a hard failure, so one slow
        // site can never stall /play for 20 s.
        let yt = async {
            timeout(
                PROVIDER_DEADLINE,
                self.run("ytsearch", query, Source::YouTube),
            )
            .await
        };
        let sc = async {
            timeout(
                PROVIDER_DEADLINE,
                self.run("scsearch", query, Source::SoundCloud),
            )
            .await
        };

        let mut collect = |label: &str, outcome: Option<Result<Vec<Track>>>| match outcome {
            Some(Ok(v)) => all.extend(v),
            Some(Err(e)) => errors.push(format!("{label}: {e}")),
            None => debug!("{label} lookup exceeded {PROVIDER_DEADLINE:?}; skipping"),
        };

        match provider {
            Provider::Auto => {
                let (yt, sc) = tokio::join!(yt, sc);
                collect("YouTube", yt.ok());
                collect("SoundCloud", sc.ok());
            }
            Provider::YouTube => collect("YouTube", yt.await.ok()),
            Provider::SoundCloud => collect("SoundCloud", sc.await.ok()),
        }

        if all.is_empty() {
            if errors.is_empty() {
                bail!("no matches found for `{query}` — try a direct link");
            }
            bail!("no matches found for `{query}` ({})", errors.join("; "));
        }
        if !errors.is_empty() {
            warn!(
                "partial search failure for {query:?}: {}",
                errors.join("; ")
            );
        }

        let ranked = rank(all, query, self.limit);
        if let Ok(mut cache) = self.cache.lock() {
            // Bound memory: keep the cache small and predictable.
            if cache.len() > 256 {
                cache.clear();
            }
            cache.insert(
                key,
                Cached {
                    at: Instant::now(),
                    tracks: ranked.clone(),
                },
            );
        }
        Ok(ranked)
    }

    /// Resolve any yt-dlp supported URL into one or many tracks (playlists included).
    pub async fn resolve(&self, url: &str, requester: &str, max: usize) -> Result<Vec<Track>> {
        let url = url.trim();
        if url.is_empty() {
            bail!("empty link");
        }
        let args = vec![
            "--no-config".to_owned(),
            "--no-warnings".to_owned(),
            "--skip-download".to_owned(),
            "--ignore-errors".to_owned(),
            "--flat-playlist".to_owned(),
            "--dump-json".to_owned(),
            "--playlist-end".to_owned(),
            max.to_string(),
            url.to_owned(),
        ];
        let out = self.spawn(&args, Duration::from_secs(15)).await?;
        let mut tracks = parse_output(&out, requester);

        // Generic audio files sometimes have no extractor metadata at all.
        // Still allow them: songbird can stream a plain HTTP audio source.
        if tracks.is_empty() && is_audio_file(url) {
            let name = url
                .split('?')
                .next()
                .unwrap_or(url)
                .rsplit('/')
                .next()
                .unwrap_or("Audio stream");
            tracks.push(Track {
                title: percent_decode(name),
                author: String::new(),
                url: url.to_owned(),
                thumbnail: None,
                duration: None,
                source: Source::Direct,
                requester: requester.to_owned(),
            });
        }
        if tracks.is_empty() {
            bail!("couldn't resolve that link — is it supported by yt-dlp?");
        }
        Ok(tracks)
    }

    async fn run(&self, prefix: &str, query: &str, source: Source) -> Result<Vec<Track>> {
        let args = vec![
            "--no-config".to_owned(),
            "--no-warnings".to_owned(),
            "--skip-download".to_owned(),
            "--ignore-errors".to_owned(),
            "--flat-playlist".to_owned(),
            "--dump-json".to_owned(),
            "--playlist-end".to_owned(),
            self.limit.to_string(),
            format!("{prefix}{}:{query}", self.limit),
        ];
        let out = self.spawn(&args, Duration::from_secs(9)).await?;
        Ok(parse_output(&out, "")
            .into_iter()
            .map(|mut t| {
                // The extractor we queried is the authoritative catalogue label.
                t.source = source.clone();
                t
            })
            .collect())
    }

    async fn spawn(&self, args: &[String], limit: Duration) -> Result<String> {
        let mut all = args.to_vec();
        // Keep the URL/query as the final argument: insert user flags before it.
        if let Some(target) = all.pop() {
            all.extend(self.extra.iter().cloned());
            all.push(target);
        }
        let output = timeout(
            limit,
            Command::new(&self.program)
                .args(&all)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("yt-dlp timed out")?
        .with_context(|| format!("failed to execute '{}'", self.program))?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if !output.status.success() && stdout.trim().is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "yt-dlp failed: {}",
                stderr.trim().chars().take(300).collect::<String>()
            );
        }
        Ok(stdout)
    }
}

fn parse_output(raw: &str, requester: &str) -> Vec<Track> {
    let mut tracks = Vec::new();
    for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        // Some extractors return a single playlist JSON object instead of lines.
        if let Some(entries) = value.get("entries").and_then(Value::as_array) {
            for entry in entries {
                if let Some(t) = parse_value(entry, requester) {
                    tracks.push(t);
                }
            }
        } else if let Some(t) = parse_value(&value, requester) {
            tracks.push(t);
        }
    }
    tracks
}

fn parse_value(v: &Value, requester: &str) -> Option<Track> {
    let title = str_field(v, &["title", "name", "track", "alt_title"])?;
    if title.is_empty() {
        return None;
    }
    let id = str_field(v, &["id"]).unwrap_or_default();
    let mut url = str_field(v, &["webpage_url", "url", "original_url"])
        .unwrap_or_default()
        .to_owned();
    if !url.starts_with("http") && !id.is_empty() {
        // Flat YouTube search entries only carry an id in some yt-dlp builds.
        url = format!("https://www.youtube.com/watch?v={id}");
    }
    if !url.starts_with("http") {
        return None;
    }

    let author = str_field(
        v,
        &["uploader", "channel", "artist", "creator", "uploader_id"],
    )
    .unwrap_or_default();
    let duration = v.get("duration").and_then(Value::as_f64);
    let extractor = str_field(v, &["extractor", "ie_key", "extractor_key"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    let source = if extractor.contains("soundcloud") {
        Source::SoundCloud
    } else if extractor.contains("youtube") {
        Source::YouTube
    } else {
        detect_source(&url, Source::Other("Web".to_owned()))
    };

    let mut thumbnail = str_field(v, &["thumbnail"]).map(|s| s.to_owned());
    if thumbnail.is_none() {
        thumbnail = v
            .get("thumbnails")
            .and_then(Value::as_array)
            .and_then(|a| a.last())
            .and_then(|t| t.get("url"))
            .and_then(Value::as_str)
            .map(|s| s.to_owned());
    }
    if thumbnail.is_none() && source == Source::YouTube && !id.is_empty() {
        thumbnail = Some(format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"));
    }

    Some(Track {
        title: crate::util::truncate(title, 120),
        author: crate::util::truncate(author, 80),
        url,
        thumbnail,
        duration,
        source,
        requester: requester.to_owned(),
    })
}

fn str_field<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn detect_source(url: &str, fallback: Source) -> Source {
    let lower = url.to_ascii_lowercase();
    if lower.contains("youtube.com") || lower.contains("youtu.be") {
        Source::YouTube
    } else if lower.contains("soundcloud.com") {
        Source::SoundCloud
    } else if is_audio_file(&lower) {
        Source::Direct
    } else if lower.contains("bandcamp.com") {
        Source::Other("Bandcamp".to_owned())
    } else if lower.contains("archive.org") {
        Source::Other("Archive".to_owned())
    } else if lower.contains("vimeo.com") {
        Source::Other("Vimeo".to_owned())
    } else if lower.contains("twitch.tv") {
        Source::Other("Twitch".to_owned())
    } else {
        fallback
    }
}

fn is_audio_file(url: &str) -> bool {
    let path = url
        .split('?')
        .next()
        .unwrap_or(url)
        .split('#')
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    [
        ".mp3", ".m4a", ".aac", ".ogg", ".opus", ".flac", ".wav", ".webm", ".mp4",
    ]
    .iter()
    .any(|ext| path.ends_with(*ext))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(a), Some(b)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(a * 16 + b);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Literal-match scoring: reward every query token that appears in the title or
/// artist, reward a full-phrase / exact-title match, and gently prefer the long
/// tail (SoundCloud) where rare tracks actually live.
fn score(query: &str, t: &Track) -> f64 {
    let q = normalize(query);
    let title = normalize(&t.title);
    let artist = normalize(&t.author);
    let hay = format!("{title} {artist}");
    let words: Vec<&str> = hay.split_whitespace().collect();
    let toks = tokens(query);
    if toks.is_empty() {
        return 0.0;
    }
    let mut hits = 0.0;
    for tok in &toks {
        if words.contains(&tok.as_str()) {
            hits += 2.0;
        } else if hay.contains(tok.as_str()) {
            hits += 1.0;
        }
    }
    let mut s = hits / (toks.len() as f64 * 2.0);
    if !q.is_empty() && title.contains(&q) {
        s += 2.0;
    }
    if !q.is_empty() && title == q {
        s += 2.5;
    }
    if matches!(t.source, Source::SoundCloud) {
        s += 0.3; // surface niche uploads instead of burying them
    }
    if t.duration.is_some_and(|d| d > 20.0) {
        s += 0.15;
    }
    let lower = t.title.to_ascii_lowercase();
    for bad in ["full album", "1 hour", "compilation", "playlist", "best of"] {
        if lower.contains(bad) {
            s -= 0.4;
        }
    }
    s
}

fn rank(mut tracks: Vec<Track>, query: &str, limit: usize) -> Vec<Track> {
    tracks.sort_by(|a, b| {
        score(query, b)
            .partial_cmp(&score(query, a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut seen_titles: HashSet<String> = HashSet::new();
    let mut seen_urls: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(limit.min(tracks.len()));
    for t in tracks {
        let key = format!("{}|{}", normalize(&t.title), normalize(&t.author));
        let key: String = key.chars().take(80).collect();
        if !seen_titles.insert(key) || !seen_urls.insert(t.url.clone()) {
            continue;
        }
        out.push(t);
        if out.len() >= limit {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_detection() {
        assert!(crate::util::looks_like_url("https://youtu.be/abc"));
        assert!(!crate::util::looks_like_url("daft punk around the world"));
    }

    #[test]
    fn scoring_prefers_exact_title() {
        let make = |title: &str, source: Source| Track {
            title: title.into(),
            author: "Nils Frahm".into(),
            url: "https://example.com/x".into(),
            thumbnail: None,
            duration: Some(200.0),
            source,
            requester: String::new(),
        };
        let exact = make("Says", Source::YouTube);
        let wrong = make("Says (Live Bootleg 2011)", Source::YouTube);
        assert!(score("Says", &exact) > score("Says", &wrong));
    }

    #[test]
    fn suggestions_reuse_prefixes_and_avoid_duplicates() {
        let s = Searcher::new("yt-dlp".into(), Vec::new(), 10, Duration::from_secs(600));
        let track = Track {
            title: "Phiriye Dao".into(),
            author: "Miles".into(),
            url: "https://www.youtube.com/watch?v=abc".into(),
            thumbnail: None,
            duration: Some(274.0),
            source: Source::YouTube,
            requester: String::new(),
        };
        {
            let mut cache = s.cache.lock().expect("cache lock");
            cache.insert(
                "YouTube|phiriye dao".into(),
                Cached {
                    at: Instant::now(),
                    tracks: vec![track.clone(), track],
                },
            );
        }
        // Every progressive keystroke hits the same cached result.
        assert_eq!(s.suggestions("phir").len(), 1);
        assert_eq!(s.suggestions("phiriye").len(), 1);
        assert_eq!(s.suggestions("phiriye dao").len(), 1);
        assert!(s.suggestions("something else").is_empty());
        assert!(s.suggestions("p").is_empty());
    }

    #[test]
    fn default_args_harden_youtube_and_bound_sockets() {
        let args = default_ytdlp_args();
        assert!(args.contains(&"--socket-timeout".to_owned()));
        assert!(args.contains(&"--retries".to_owned()));
        assert!(args.iter().any(|a| a.contains("youtube:player_client")));
    }

    #[test]
    fn parses_flat_json() {
        let raw = r#"{"id":"abc","title":"Rare Track","url":"https://www.youtube.com/watch?v=abc","duration":123.0,"channel":"Small Artist"}"#;
        let parsed = parse_output(raw, "tester");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].title, "Rare Track");
        assert_eq!(parsed[0].author, "Small Artist");
        assert!(parsed[0].thumbnail.is_some());
    }
}
