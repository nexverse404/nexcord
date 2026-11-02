//! Per-guild playback state, the queue, and the voice lifecycle.
//!
//! No built-in queue framework is used: a small `VecDeque<Track>` plus one
//! global `TrackEvent::End` listener gives us exact control over loop modes,
//! shuffle, seeking and idle disconnect with almost no overhead.

use crate::{
    config::Config,
    emojis,
    search::{Searcher, Source, Track},
    ui::{self, Assets},
    util,
};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serenity::{
    builder::{CreateActionRow, CreateEmbed, CreateMessage, EditMessage},
    client::Context,
    http::Http,
    model::prelude::{ChannelId, GuildId, MessageId, UserId},
};
use songbird::{
    driver::Bitrate,
    input::{HttpRequest, Input, YoutubeDl},
    tracks::TrackHandle,
    Event as VoiceEvent, EventContext, EventHandler as VoiceEventHandler, Songbird, TrackEvent,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopMode {
    Off,
    Track,
    Queue,
}

impl LoopMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Track => "Track",
            Self::Queue => "Queue",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::Track,
            Self::Track => Self::Queue,
            Self::Queue => Self::Off,
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "track" | "one" => Self::Track,
            "queue" | "all" => Self::Queue,
            _ => Self::Off,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PanelRef {
    pub channel_id: ChannelId,
    pub message_id: MessageId,
}

pub struct GuildState {
    pub queue: VecDeque<Track>,
    pub current: Option<Track>,
    pub handle: Option<TrackHandle>,
    /// Guards against stale `TrackEvent::End` events from skipped tracks.
    pub current_uuid: Option<String>,
    pub loading: bool,
    pub position: Duration,
    pub volume: f32,
    pub paused: bool,
    pub loop_mode: LoopMode,
    pub panel: Option<PanelRef>,
    pub text_channel: Option<ChannelId>,
    pub voice_channel: Option<ChannelId>,
    pub queue_page: usize,
    /// Monotonic stamp used to coalesce concurrent panel refreshes so a burst
    /// of button presses cannot burn the message-edit rate limit.
    pub panel_gen: u64,
    pub last_active: Instant,
}

impl GuildState {
    fn new(volume: f32) -> Self {
        Self {
            queue: VecDeque::new(),
            current: None,
            handle: None,
            current_uuid: None,
            loading: false,
            position: Duration::ZERO,
            volume,
            paused: false,
            loop_mode: LoopMode::Off,
            panel: None,
            text_channel: None,
            voice_channel: None,
            queue_page: 0,
            panel_gen: 0,
            last_active: Instant::now(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub current: Option<Track>,
    pub queue: Vec<Track>,
    pub queue_len: usize,
    pub volume: f32,
    pub paused: bool,
    pub loop_mode: LoopMode,
    pub position: Duration,
    pub connected: bool,
    pub text_channel: Option<ChannelId>,
    pub panel: Option<PanelRef>,
}

#[derive(Clone, Debug)]
pub struct SearchSession {
    pub at: Instant,
    pub guild_id: GuildId,
    pub user_id: UserId,
    pub text_channel: ChannelId,
    pub tracks: Vec<Track>,
}

pub struct EnqueueResult {
    pub added: usize,
    pub position: usize,
    pub first: Track,
}

pub struct AppState {
    pub config: Config,
    pub http: Arc<Http>,
    pub voice: Arc<Songbird>,
    pub client: reqwest::Client,
    pub searcher: Searcher,
    pub assets: Assets,
    pub started: Instant,
    /// Caps background autocomplete searches so rapid typing can't spawn a
    /// stampede of yt-dlp processes.
    pub inflight: AtomicUsize,
    guilds: RwLock<HashMap<GuildId, Arc<Mutex<GuildState>>>>,
    sessions: StdMutex<HashMap<String, SearchSession>>,
    seq: AtomicU64,
}

impl AppState {
    pub fn new(config: Config, http: Arc<Http>, voice: Arc<Songbird>) -> Self {
        let searcher = Searcher::new(
            config.yt_dlp.clone(),
            config.search_results,
            config.search_cache_ttl,
        );
        let assets = Assets::new(config.asset_base_url.clone());
        Self {
            config,
            http,
            voice,
            client: reqwest::Client::new(),
            searcher,
            assets,
            started: Instant::now(),
            inflight: AtomicUsize::new(0),
            guilds: RwLock::new(HashMap::new()),
            sessions: StdMutex::new(HashMap::new()),
            seq: AtomicU64::new(1),
        }
    }

    pub async fn guild(&self, guild_id: GuildId) -> Arc<Mutex<GuildState>> {
        if let Some(found) = self.guilds.read().await.get(&guild_id) {
            return found.clone();
        }
        let mut map = self.guilds.write().await;
        map.entry(guild_id)
            .or_insert_with(|| Arc::new(Mutex::new(GuildState::new(self.config.default_volume))))
            .clone()
    }

    // ── search sessions ────────────────────────────────────────────────────

    pub fn new_session(&self, session: SearchSession) -> String {
        let id = format!("s{}", self.seq.fetch_add(1, Ordering::Relaxed));
        if let Ok(mut map) = self.sessions.lock() {
            if map.len() > 256 {
                map.clear();
            }
            map.insert(id.clone(), session);
        }
        id
    }

    pub fn take_session(&self, id: &str) -> Option<SearchSession> {
        let session = self.sessions.lock().ok()?.remove(id)?;
        (session.at.elapsed() < Duration::from_secs(600)).then_some(session)
    }

    fn purge_sessions(&self) {
        if let Ok(mut map) = self.sessions.lock() {
            map.retain(|_, s| s.at.elapsed() < Duration::from_secs(600));
        }
    }

    pub fn session_count(&self) -> usize {
        self.sessions.lock().map(|m| m.len()).unwrap_or(0)
    }

    // ── voice ──────────────────────────────────────────────────────────────

    /// Join the invoker's channel (or move to it), configure high-quality audio,
    /// and install a single global end-of-track listener for this call.
    pub async fn ensure_voice(
        app: &Arc<Self>,
        ctx: &Context,
        guild_id: GuildId,
        user_id: UserId,
    ) -> Result<ChannelId> {
        let channel_id = {
            let guild = ctx
                .cache
                .guild(guild_id)
                .ok_or_else(|| anyhow!("guild isn't cached yet — try again in a moment"))?;
            guild
                .voice_states
                .get(&user_id)
                .and_then(|state| state.channel_id)
        }
        .ok_or_else(|| anyhow!("join a voice channel first, then run the command"))?;

        let current = app.voice.get(guild_id);
        let already_there = match &current {
            Some(call) => {
                call.lock().await.current_channel().map(|c| c.0.get()) == Some(channel_id.get())
            }
            None => false,
        };

        if !already_there {
            let call = app
                .voice
                .join(guild_id, channel_id)
                .await
                .map_err(|e| anyhow!("couldn't join voice: {e:?}"))?;
            let mut handler = call.lock().await;
            if let Err(e) = handler.deafen(true).await {
                debug!("deafen failed (harmless): {e:?}");
            }
            // Stereo Opus at an explicit high bitrate. Songbird's YoutubeDl source
            // chooses the best audio-only stream (usually WebM/Opus), so most
            // tracks are passed through without a lossy re-encode.
            handler.set_bitrate(Bitrate::BitsPerSecond(app.config.opus_bitrate));
            handler.add_global_event(
                VoiceEvent::Track(TrackEvent::End),
                TrackEndHandler {
                    app: Arc::clone(app),
                    guild_id,
                },
            );
            info!(%guild_id, %channel_id, "joined voice");
        }

        let state = app.guild(guild_id).await;
        state.lock().await.voice_channel = Some(channel_id);
        Ok(channel_id)
    }

    // ── queue / playback ───────────────────────────────────────────────────

    /// Join (if needed), append tracks, start playback if idle, refresh the panel.
    pub async fn enqueue(
        app: &Arc<Self>,
        ctx: &Context,
        guild_id: GuildId,
        user_id: UserId,
        requester: &str,
        mut tracks: Vec<Track>,
        text_channel: ChannelId,
    ) -> Result<EnqueueResult> {
        if tracks.is_empty() {
            bail!("nothing to play");
        }
        let voice_channel = Self::ensure_voice(app, ctx, guild_id, user_id).await?;
        let state = app.guild(guild_id).await;
        let mut st = state.lock().await;
        for t in tracks.iter_mut() {
            t.requester = requester.to_owned();
        }
        let free = app.config.max_queue.saturating_sub(st.queue.len());
        tracks.truncate(free.min(tracks.len()));
        if tracks.is_empty() {
            bail!("the queue is full ({})", app.config.max_queue);
        }
        let was_idle = st.current.is_none() && st.handle.is_none() && !st.loading;
        for t in &tracks {
            st.queue.push_back(t.clone());
        }
        let position = if was_idle {
            0
        } else {
            st.queue.len() - tracks.len() + 1
        };
        st.text_channel = Some(text_channel);
        st.voice_channel = Some(voice_channel);
        st.last_active = Instant::now();
        drop(st);

        let first = tracks[0].clone();
        let added = tracks.len();
        if was_idle {
            app.start_next(guild_id).await?;
        }
        app.refresh_panel(guild_id).await;
        Ok(EnqueueResult {
            added,
            position,
            first,
        })
    }

    /// Pop the next track and hand a lazy `YoutubeDl` source to songbird.
    /// yt-dlp isn't actually invoked until the mixer needs the stream.
    async fn start_next(&self, guild_id: GuildId) -> Result<()> {
        let (track, volume) = {
            let state = self.guild(guild_id).await;
            let mut st = state.lock().await;
            if st.loading || st.handle.is_some() || st.current.is_some() {
                return Ok(());
            }
            let Some(track) = st.queue.pop_front() else {
                st.position = Duration::ZERO;
                return Ok(());
            };
            st.current = Some(track.clone());
            st.loading = true;
            (track, st.volume)
        };

        let call = self
            .voice
            .get(guild_id)
            .ok_or_else(|| anyhow!("not connected to voice"))?;
        // Plain audio URLs are streamed directly (no extractor needed).
        // Everything else goes through yt-dlp's best-audio-only selection.
        let input: Input = if track.source == Source::Direct {
            HttpRequest::new(self.client.clone(), track.url.clone()).into()
        } else {
            let source: YoutubeDl<'static> = YoutubeDl::new(self.client.clone(), track.url.clone());
            source.into()
        };
        let handle = {
            let mut handler = call.lock().await;
            handler.play_input(input)
        };
        let _ = handle.set_volume(volume);

        {
            let state = self.guild(guild_id).await;
            let mut st = state.lock().await;
            st.current_uuid = Some(handle.uuid().to_string());
            st.handle = Some(handle);
            st.loading = false;
            st.paused = false;
            st.position = Duration::ZERO;
            st.last_active = Instant::now();
        }
        self.refresh_panel(guild_id).await;
        Ok(())
    }

    /// Fired from a spawned task, never from inside the audio mixer itself.
    pub async fn on_track_end(&self, guild_id: GuildId, uuid: &str) {
        let state = self.guild(guild_id).await;
        let has_next = {
            let mut st = state.lock().await;
            if st.current_uuid.as_deref() != Some(uuid) {
                debug!(%guild_id, "ignoring stale track-end event");
                return;
            }
            if let Some(track) = st.current.clone() {
                match st.loop_mode {
                    LoopMode::Track => st.queue.push_front(track),
                    LoopMode::Queue => st.queue.push_back(track),
                    LoopMode::Off => {}
                }
            }
            st.current = None;
            st.handle = None;
            st.current_uuid = None;
            st.loading = false;
            st.paused = false;
            st.position = Duration::ZERO;
            st.last_active = Instant::now();
            !st.queue.is_empty()
        };

        if has_next {
            if let Err(e) = self.start_next(guild_id).await {
                warn!(%guild_id, "failed to start next track: {e}");
            }
        } else {
            self.refresh_panel(guild_id).await;
        }
    }

    pub async fn skip(&self, guild_id: GuildId, to: Option<usize>) -> Result<()> {
        let state = self.guild(guild_id).await;
        {
            let mut st = state.lock().await;
            if let Some(target) = to {
                if target > 1 {
                    let drop = (target - 1).min(st.queue.len());
                    for _ in 0..drop {
                        st.queue.pop_front();
                    }
                }
            }
            // Clear the uuid BEFORE stopping so the resulting End event is ignored.
            st.current_uuid = None;
            if let Some(handle) = st.handle.take() {
                let _ = handle.stop();
            }
            st.current = None;
            st.loading = false;
            st.paused = false;
            st.position = Duration::ZERO;
            st.last_active = Instant::now();
        }
        self.start_next(guild_id).await?;
        self.refresh_panel(guild_id).await;
        Ok(())
    }

    pub async fn stop(&self, guild_id: GuildId) -> Result<()> {
        let state = self.guild(guild_id).await;
        {
            let mut st = state.lock().await;
            st.current_uuid = None;
            if let Some(handle) = st.handle.take() {
                let _ = handle.stop();
            }
            st.current = None;
            st.queue.clear();
            st.loading = false;
            st.paused = false;
            st.position = Duration::ZERO;
            st.last_active = Instant::now();
        }
        self.refresh_panel(guild_id).await;
        Ok(())
    }

    pub async fn toggle_pause(&self, guild_id: GuildId) -> Result<bool> {
        let state = self.guild(guild_id).await;
        let (handle, paused) = {
            let st = state.lock().await;
            let handle = st
                .handle
                .clone()
                .ok_or_else(|| anyhow!("nothing is playing"))?;
            (handle, !st.paused)
        };
        let outcome = if paused {
            handle.pause()
        } else {
            handle.play()
        };
        outcome.map_err(|e| anyhow!("playback control failed: {e:?}"))?;
        {
            let mut st = state.lock().await;
            st.paused = paused;
            st.last_active = Instant::now();
        }
        self.refresh_panel(guild_id).await;
        Ok(paused)
    }

    pub async fn set_volume(&self, guild_id: GuildId, percent: u8) -> Result<f32> {
        let value = (percent.min(200) as f32 / 100.0).clamp(0.0, 2.0);
        let state = self.guild(guild_id).await;
        let mut st = state.lock().await;
        st.volume = value;
        if let Some(handle) = &st.handle {
            handle
                .set_volume(value)
                .map_err(|e| anyhow!("volume failed: {e:?}"))?;
        }
        st.last_active = Instant::now();
        drop(st);
        self.refresh_panel(guild_id).await;
        Ok(value)
    }

    pub async fn nudge_volume(&self, guild_id: GuildId, delta: i32) -> Result<f32> {
        let current = {
            let state = self.guild(guild_id).await;
            let st = state.lock().await;
            (st.volume * 100.0).round() as i32
        };
        let percent = (current + delta).clamp(0, 200) as u8;
        self.set_volume(guild_id, percent).await
    }

    pub async fn set_loop(&self, guild_id: GuildId, mode: LoopMode) -> Result<()> {
        let state = self.guild(guild_id).await;
        state.lock().await.loop_mode = mode;
        self.refresh_panel(guild_id).await;
        Ok(())
    }

    pub async fn cycle_loop(&self, guild_id: GuildId) -> Result<LoopMode> {
        let state = self.guild(guild_id).await;
        let mode = {
            let mut st = state.lock().await;
            st.loop_mode = st.loop_mode.cycle();
            st.loop_mode
        };
        self.refresh_panel(guild_id).await;
        Ok(mode)
    }

    pub async fn shuffle(&self, guild_id: GuildId) -> Result<usize> {
        let state = self.guild(guild_id).await;
        let mut st = state.lock().await;
        let mut items: Vec<Track> = st.queue.drain(..).collect();
        util::shuffle(&mut items);
        let count = items.len();
        st.queue.extend(items);
        st.last_active = Instant::now();
        drop(st);
        self.refresh_panel(guild_id).await;
        Ok(count)
    }

    pub async fn seek(&self, guild_id: GuildId, position: Duration) -> Result<Duration> {
        let state = self.guild(guild_id).await;
        let handle = state
            .lock()
            .await
            .handle
            .clone()
            .ok_or_else(|| anyhow!("nothing is playing"))?;
        let result = handle
            .seek_async(position)
            .await
            .map_err(|e| anyhow!("seek failed (this source may not support it): {e:?}"))?;
        self.refresh_panel(guild_id).await;
        Ok(result)
    }

    pub async fn seek_relative(&self, guild_id: GuildId, seconds: i64) -> Result<()> {
        let state = self.guild(guild_id).await;
        let handle = state
            .lock()
            .await
            .handle
            .clone()
            .ok_or_else(|| anyhow!("nothing is playing"))?;
        let info = handle
            .get_info()
            .await
            .map_err(|e| anyhow!("can't read position: {e:?}"))?;
        let current = info.position.as_secs() as i64;
        let target = (current + seconds).max(0) as u64;
        handle
            .seek_async(Duration::from_secs(target))
            .await
            .map_err(|e| anyhow!("seek failed: {e:?}"))?;
        self.refresh_panel(guild_id).await;
        Ok(())
    }

    pub async fn remove_at(&self, guild_id: GuildId, position: usize) -> Result<Track> {
        let state = self.guild(guild_id).await;
        let removed = {
            let mut st = state.lock().await;
            if position == 0 || position > st.queue.len() {
                bail!("there's no track at position {position}");
            }
            st.queue.remove(position - 1)
        };
        let removed = removed.ok_or_else(|| anyhow!("there's no track at position {position}"))?;
        self.refresh_panel(guild_id).await;
        Ok(removed)
    }

    pub async fn leave(&self, guild_id: GuildId) -> Result<()> {
        self.voice
            .remove(guild_id)
            .await
            .map_err(|e| anyhow!("couldn't leave voice: {e:?}"))?;
        let state = self.guild(guild_id).await;
        {
            let mut st = state.lock().await;
            st.current_uuid = None;
            if let Some(handle) = st.handle.take() {
                let _ = handle.stop();
            }
            st.current = None;
            st.queue.clear();
            st.paused = false;
            st.loading = false;
            st.voice_channel = None;
            st.position = Duration::ZERO;
            st.last_active = Instant::now();
        }
        self.refresh_panel(guild_id).await;
        Ok(())
    }

    pub async fn set_text_channel(&self, guild_id: GuildId, channel: ChannelId) {
        let state = self.guild(guild_id).await;
        state.lock().await.text_channel = Some(channel);
    }

    pub async fn set_queue_page(&self, guild_id: GuildId, page: usize) {
        let state = self.guild(guild_id).await;
        state.lock().await.queue_page = page;
    }

    pub async fn queue_page(&self, guild_id: GuildId) -> usize {
        let state = self.guild(guild_id).await;
        let st = state.lock().await;
        st.queue_page
    }

    // ── UI state ───────────────────────────────────────────────────────────

    pub async fn snapshot(&self, guild_id: GuildId) -> Snapshot {
        let state = self.guild(guild_id).await;
        let (
            current,
            queue,
            queue_len,
            volume,
            paused,
            loop_mode,
            handle,
            mut position,
            connected,
            text_channel,
            panel,
        ) = {
            let st = state.lock().await;
            (
                st.current.clone(),
                st.queue.iter().take(60).cloned().collect::<Vec<_>>(),
                st.queue.len(),
                st.volume,
                st.paused,
                st.loop_mode,
                st.handle.clone(),
                st.position,
                st.voice_channel.is_some(),
                st.text_channel,
                st.panel,
            )
        };
        if let Some(handle) = handle {
            if let Ok(info) = handle.get_info().await {
                position = info.position;
                let mut st = state.lock().await;
                st.position = position;
            }
        }
        Snapshot {
            current,
            queue,
            queue_len,
            volume,
            paused,
            loop_mode,
            position,
            connected,
            text_channel,
            panel,
        }
    }

    /// Edit the persistent panel in place. Discord keeps existing attachments
    /// when the `attachments` field is omitted, so the artwork is uploaded once.
    pub async fn refresh_panel(&self, guild_id: GuildId) {
        let generation = {
            let state = self.guild(guild_id).await;
            let mut st = state.lock().await;
            st.panel_gen = st.panel_gen.wrapping_add(1);
            st.panel_gen
        };
        let snap = self.snapshot(guild_id).await;
        // If a newer refresh started while we were building, it carries the
        // fresher state and will do the edit — drop this one.
        let state = self.guild(guild_id).await;
        if state.lock().await.panel_gen != generation {
            return;
        }
        let embed = ui::panel_embed(&snap, &self.assets, self.config.accent);
        let components = ui::panel_components(&snap);
        // The snapshot already carries the panel's own channel and message, so
        // no second mutex acquisition is needed here.
        let channel = snap.text_channel;

        if let Some(panel) = snap.panel {
            let edit = EditMessage::new()
                .embed(embed.clone())
                .components(components.clone());
            match panel
                .channel_id
                .edit_message(self.http.clone(), panel.message_id, edit)
                .await
            {
                Ok(_) => return,
                Err(e) => debug!(%guild_id, "panel message gone, recreating: {e:?}"),
            }
        }
        if let Some(channel) = channel {
            self.post_panel(guild_id, channel, embed, components).await;
        }
    }

    async fn post_panel(
        &self,
        guild_id: GuildId,
        channel: ChannelId,
        embed: CreateEmbed,
        components: Vec<CreateActionRow>,
    ) {
        let mut message = CreateMessage::new().embed(embed).components(components);
        for attachment in self.assets.attachments() {
            message = message.add_file(attachment);
        }
        match channel.send_message(self.http.clone(), message).await {
            Ok(sent) => {
                let state = self.guild(guild_id).await;
                state.lock().await.panel = Some(PanelRef {
                    channel_id: channel,
                    message_id: sent.id,
                });
            }
            Err(e) => warn!(%guild_id, "couldn't post control panel: {e:?}"),
        }
    }

    pub async fn reset_panel(&self, guild_id: GuildId) {
        let state = self.guild(guild_id).await;
        state.lock().await.panel = None;
        self.refresh_panel(guild_id).await;
    }

    // ── housekeeping ───────────────────────────────────────────────────────

    pub async fn housekeeping(&self) {
        self.searcher.purge();
        self.purge_sessions();
        if self.config.idle_timeout.is_zero() {
            return;
        }
        let ids: Vec<GuildId> = self.guilds.read().await.keys().copied().collect();
        for guild_id in ids {
            let state = self.guild(guild_id).await;
            let idle = {
                let st = state.lock().await;
                st.voice_channel.is_some()
                    && st.current.is_none()
                    && st.queue.is_empty()
                    && !st.loading
                    && st.last_active.elapsed() > self.config.idle_timeout
            };
            if idle {
                let _ = self.voice.remove(guild_id).await;
                state.lock().await.voice_channel = None;
                self.refresh_panel(guild_id).await;
                info!(%guild_id, "auto-left an idle voice channel");
            }
        }
    }

    pub async fn stats(&self) -> Vec<(String, String, bool)> {
        let states: Vec<Arc<Mutex<GuildState>>> =
            self.guilds.read().await.values().cloned().collect();
        let mut playing = 0usize;
        let mut queued = 0usize;
        let mut connected = 0usize;
        for state in states {
            let st = state.lock().await;
            if st.current.is_some() {
                playing += 1;
            }
            if st.voice_channel.is_some() {
                connected += 1;
            }
            queued += st.queue.len();
        }
        let uptime = self.started.elapsed();
        vec![
            ("Uptime".into(), util::fmt_duration(uptime), true),
            ("Voice sessions".into(), connected.to_string(), true),
            ("Playing now".into(), playing.to_string(), true),
            ("Tracks queued".into(), queued.to_string(), true),
            (
                "Search cache".into(),
                self.searcher.cache_len().to_string(),
                true,
            ),
            (
                "Open pickers".into(),
                self.session_count().to_string(),
                true,
            ),
            (
                "Custom emojis".into(),
                emojis::custom_count().to_string(),
                true,
            ),
            (
                "Audio".into(),
                format!(
                    "48 kHz stereo Opus · {} kbps",
                    self.config.opus_bitrate / 1000
                ),
                false,
            ),
            (
                "Engine".into(),
                "serenity + songbird + yt-dlp · 0 API keys".into(),
                false,
            ),
        ]
    }
}

struct TrackEndHandler {
    app: Arc<AppState>,
    guild_id: GuildId,
}

#[async_trait]
impl VoiceEventHandler for TrackEndHandler {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<VoiceEvent> {
        let ended = match ctx {
            EventContext::Track(list) => list.last().map(|pair| pair.1.uuid().to_string()),
            _ => None,
        };
        if let Some(uuid) = ended {
            let app = self.app.clone();
            let guild_id = self.guild_id;
            // Never do I/O or lock the driver from inside the mixer tick.
            tokio::spawn(async move {
                app.on_track_end(guild_id, &uuid).await;
            });
        }
        None
    }
}
