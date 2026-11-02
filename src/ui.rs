//! All embed + component construction lives here, so the design stays in one
//! place and the rest of the bot never deals with Discord's JSON shapes.
//!
//! Aesthetic: dark, low-chroma surfaces; one violet/cyan accent; monospace
//! metadata; short labels; no decorative clutter. The generated artwork is
//! embedded at compile time (~100 KB total) and uploaded with the panel message
//! via `attachment://`, or referenced from a CDN when `ASSET_BASE_URL` is set.

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

    /// Only needed for messages whose embeds reference `attachment://…`.
    pub fn attachments(&self) -> Vec<CreateAttachment> {
        if !self.needs_upload() {
            return Vec::new();
        }
        vec![
            CreateAttachment::bytes(MARK.to_vec(), MARK_FILE),
            CreateAttachment::bytes(WAVE.to_vec(), WAVE_FILE),
            CreateAttachment::bytes(COVER.to_vec(), COVER_FILE),
        ]
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

pub fn notice(title: &str, description: &str, tone: Tone) -> CreateEmbed {
    CreateEmbed::new()
        .title(format!("{}  {}", emojis::mention(tone.key()), title))
        .description(description)
        .color(tone.colour())
        .footer(CreateEmbedFooter::new("N E X C O R D  ·  keep it simple"))
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

/// The persistent music control panel.
pub fn panel_embed(snap: &Snapshot, assets: &Assets, accent: u32) -> CreateEmbed {
    let idle = snap.current.is_none();
    let colour = if idle {
        Colour::new(0x2b2f3a)
    } else if snap.paused {
        Colour::new(0xf5a524)
    } else {
        Colour::new(accent)
    };

    let mut embed = CreateEmbed::new()
        .author(
            CreateEmbedAuthor::new("N E X C O R D   /   CONTROL PANEL")
                .icon_url(assets.get(MARK_FILE)),
        )
        .color(colour)
        .image(assets.get(WAVE_FILE))
        .thumbnail(
            snap.current
                .as_ref()
                .and_then(|t| t.thumbnail.clone())
                .unwrap_or_else(|| assets.get(COVER_FILE)),
        );

    if let Some(track) = snap.current.as_ref() {
        let total = track
            .duration
            .filter(|d| d.is_finite() && *d > 0.0)
            .map(Duration::from_secs_f64);
        let artist = if track.author.is_empty() {
            "Unknown artist"
        } else {
            &track.author
        };
        let status = format!(
            "{}  **{}**",
            emojis::mention(if snap.paused { "paused" } else { "playing" }),
            if snap.paused { "Paused" } else { "Playing" }
        );
        let connection = format!(
            "{} {}",
            emojis::mention(if snap.connected {
                "connected"
            } else {
                "offline"
            }),
            if snap.connected {
                "connected"
            } else {
                "offline"
            }
        );
        // Named arguments keep the ten placeholders auditable against the
        // Discord embed limits (description <= 4096 chars).
        let description = format!(
            "> {status}   ·   {connection}\n\
             > **{artist}**  ·  `{duration}`  ·  {source_icon}\n>\n\
             > `{position}`   {bar}   `{total}`\n>\n\
             > `VOL` **{volume:>3}%**   ·   `LOOP` **{loop_mode}**   ·   `UP NEXT` **{up_next}**",
            artist = util::clean_md(&util::truncate(artist, 64)),
            duration = util::fmt_secs(track.duration),
            source_icon = emojis::mention(track.source.emoji_key()),
            position = util::fmt_duration(snap.position),
            bar = util::progress_bar(snap.position, total, 14),
            total = util::fmt_duration(total.unwrap_or_default()),
            volume = (snap.volume * 100.0).round() as i32,
            loop_mode = snap.loop_mode.label(),
            up_next = snap.queue_len,
        );
        embed = embed
            .title(util::truncate(&track.title, 100))
            .url(track.url.clone())
            .description(description);

        if !snap.queue.is_empty() {
            let mut lines = String::new();
            for (i, next) in snap.queue.iter().take(3).enumerate() {
                lines.push_str(&format!(
                    "`{:>2}.`  {}  ·  `{}`\n",
                    i + 1,
                    util::clean_md(&util::truncate(&next.title, 58)),
                    util::fmt_secs(next.duration)
                ));
            }
            if snap.queue_len > 3 {
                lines.push_str(&format!("… and **{}** more", snap.queue_len - 3));
            }
            embed = embed.field("UP NEXT", lines, false);
        }
        embed = embed.footer(
            CreateEmbedFooter::new(format!(
                "requested by {}   ·   48 kHz stereo Opus   ·   no API keys",
                util::truncate(&track.requester, 40)
            ))
            .icon_url(assets.get(MARK_FILE)),
        );
    } else {
        embed = embed
            .title("Nothing playing")
            .description(format!(
                "> {}  **Idle**\n>\n\
                 > `/play` a song, a link or a playlist — or use `/search` to pick an exact match.\n\
                 > Works with YouTube, SoundCloud, Bandcamp and 1000+ other sites. No API key needed.",
                emojis::mention("idle")
            ))
            .footer(
                CreateEmbedFooter::new("N E X C O R D   ·   fast · clean · keyless")
                    .icon_url(assets.get(MARK_FILE)),
            );
    }

    embed
}

pub fn panel_components(snap: &Snapshot) -> Vec<CreateActionRow> {
    let playing = snap.current.is_some();
    let play_slot = if snap.paused { "play" } else { "pause" };
    let play_label = if snap.paused { "Resume" } else { "Pause" };
    let loop_label = snap.loop_mode.label();

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
                loop_label,
                ButtonStyle::Secondary,
                false,
            ),
        ]),
        CreateActionRow::Buttons(vec![
            button(
                "nx:voldown".into(),
                "volume_down",
                "−",
                ButtonStyle::Secondary,
                false,
            ),
            button(
                "nx:volup".into(),
                "volume_up",
                "+",
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
        .author(CreateEmbedAuthor::new("N E X C O R D   /   QUEUE"))
        .color(Colour::new(ACCENT_FALLBACK));

    if let Some(current) = snap.current.as_ref() {
        embed = embed.field(
            "NOW PLAYING",
            format!(
                "**{}**\n{}  ·  `{}`",
                util::clean_md(&util::truncate(&current.title, 90)),
                util::clean_md(&util::truncate(&current.author, 50)),
                util::fmt_secs(current.duration)
            ),
            false,
        );
    }

    if snap.queue.is_empty() {
        embed = embed.description(
            "> The queue is empty.\n> `/play` something, or `/search` for an exact match.",
        );
    } else {
        let start = (page * QUEUE_PER_PAGE).min(snap.queue.len());
        let end = (start + QUEUE_PER_PAGE).min(snap.queue.len());
        let mut lines = String::new();
        for (i, t) in snap.queue[start..end].iter().enumerate() {
            lines.push_str(&format!(
                "`{:>2}.`  **{}**\n      {}  ·  `{}`  ·  {}\n",
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
        "page {}/{}   ·   {} queued   ·   {} remaining   ·   {} loop",
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
    let mut lines = format!(
        "> {}  **{}**\n> {}  ·  {} matches  ·  pick the exact one\n\n",
        emojis::mention("search"),
        util::clean_md(&util::truncate(query, 90)),
        provider.label(),
        tracks.len()
    );
    for (i, t) in tracks.iter().enumerate() {
        lines.push_str(&format!(
            "`{:>2}.`  [{}]({})\n      {}\n",
            i + 1,
            util::clean_md(&util::truncate(&t.title, 74)),
            t.url,
            util::clean_md(&util::truncate(&t.subtitle(), 82))
        ));
    }
    CreateEmbed::new()
        .author(CreateEmbedAuthor::new("N E X C O R D   /   SEARCH"))
        .title("Choose a track")
        .description(util::truncate(&lines, 3900))
        .color(Colour::new(ACCENT_FALLBACK))
        .footer(CreateEmbedFooter::new(
            "exact matches · no API keys · expires in 10 min",
        ))
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
        format!("{}  Playing now", emojis::mention("playing"))
    } else {
        format!(
            "{}  Added to queue  ·  position `{position}`",
            emojis::mention("added")
        )
    };
    CreateEmbed::new()
        .author(CreateEmbedAuthor::new("N E X C O R D   /   QUEUED"))
        .title(util::truncate(&track.title, 100))
        .url(track.url.clone())
        .description(format!(
            "> {state}\n\
             > **{}**  ·  `{}`  ·  {}\n>\n\
             > requested by **{}**",
            util::clean_md(&util::truncate(&track.author, 64)),
            util::fmt_secs(track.duration),
            track.source.label(),
            util::clean_md(&util::truncate(&track.requester, 40))
        ))
        .color(Colour::new(0x3ddc97))
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
            let total = track
                .duration
                .filter(|d| d.is_finite() && *d > 0.0)
                .map(Duration::from_secs_f64);
            let status = format!(
                "{}  **{}**",
                emojis::mention(if snap.paused { "paused" } else { "playing" }),
                if snap.paused { "Paused" } else { "Playing" }
            );
            let mut embed = CreateEmbed::new()
                .author(CreateEmbedAuthor::new("N E X C O R D   /   NOW PLAYING"))
                .title(util::truncate(&track.title, 100))
                .url(track.url.clone())
                .description(format!(
                    "> {status}  ·  **{artist}**\n\
                     > `{duration}`  ·  {source}\n>\n\
                     > `{position}`   {bar}   `{total}`\n>\n\
                     > `VOL` **{volume}%**   ·   `LOOP` **{loop_mode}**   ·   `UP NEXT` **{up_next}**",
                    artist = util::clean_md(&util::truncate(artist, 64)),
                    duration = util::fmt_secs(track.duration),
                    source = track.source.label(),
                    position = util::fmt_duration(snap.position),
                    bar = util::progress_bar(snap.position, total, 14),
                    total = util::fmt_duration(total.unwrap_or_default()),
                    volume = (snap.volume * 100.0).round() as i32,
                    loop_mode = snap.loop_mode.label(),
                    up_next = snap.queue_len,
                ))
                .color(Colour::new(ACCENT_FALLBACK))
                .footer(CreateEmbedFooter::new(format!(
                    "requested by {}",
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
        .author(CreateEmbedAuthor::new("N E X C O R D   /   HELP"))
        .title("Everything, in one place")
        .description("> A keyless music bot. Type a song name, paste a link, or pick an exact result.")
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
            (
                "INFO",
                "`/now` `/stats` `/help`",
                false,
            ),
        ])
        .footer(CreateEmbedFooter::new("built in Rust · serenity + songbird · 48 kHz stereo Opus"))
}

pub fn stats_embed(fields: Vec<(String, String, bool)>) -> CreateEmbed {
    CreateEmbed::new()
        .author(CreateEmbedAuthor::new("N E X C O R D   /   STATUS"))
        .title("Runtime")
        .color(Colour::new(ACCENT_FALLBACK))
        .fields(fields)
        .footer(CreateEmbedFooter::new("lightweight by design"))
}
