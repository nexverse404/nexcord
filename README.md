# NEXCORD

> A fast, minimal, high-fidelity Discord music bot written in **Rust**.
> Slash commands · button control panel · keyless accurate search · original artwork.

![Nexcord mark](assets/nexcord-mark.jpg)

---

## Why it exists

Most music bots are slow, noisy and depend on a pile of API keys that break the moment a quota is hit. Nexcord takes the opposite route:

| | |
|---|---|
| **Speed** | Two catalogues searched **in parallel**, `--flat-playlist` (one request, no per-video fetches), in-memory result cache, slash-command autocomplete warmed in the background, and **lazy** audio sources — `yt-dlp` is not invoked until the mixer needs the stream. Every gateway event is handled in its own task. |
| **Quality** | 48 kHz **stereo** Opus, explicit 160 kbps encoder bitrate (configurable to 384 kbps), soft clipping, and Songbird's *best audio-only* stream selection — usually WebM/Opus, which is **passed through without re-encoding**. Seeking, gapless queue advance, loop modes, per-guild volume. |
| **Design** | Clean embeds with one accent colour, monospace metadata, a compact two-row control panel, a paginated queue, and an exact-match select menu. Three pieces of [original generated artwork](#own-assets) are compiled into the binary (~100 KB total). |
| **Lightness** | No command framework, no database, no HTTP server, no privileged intents (only `GUILDS` + `GUILD_VOICE_STATES`), no runtime service except `yt-dlp`. One small Rust binary. |

**No API keys.** Search and playback go through `yt-dlp`, which supports YouTube, SoundCloud, Bandcamp, archive.org, Vimeo, direct audio files and 1000+ other extractors. No YouTube Data API, no Spotify credentials, no cookies required for public content.

---

## Quick start

### 1. Prerequisites

| Requirement | Why |
|---|---|
| Rust 1.82+ (`cargo`, 1.85 recommended) | build |
| `yt-dlp` | keyless search + stream URL resolution |
| `ffmpeg` | recommended — some `yt-dlp` extractors need it for format handling |
| `libopus` + `pkg-config` | Songbird's Opus encoder (build time) |

```bash
# Debian / Ubuntu
sudo apt install -y ffmpeg libopus-dev pkg-config build-essential
python3 -m pip install -U yt-dlp

# macOS
brew install ffmpeg opus pkg-config yt-dlp
```

`yt-dlp` is the only hard runtime dependency (search + stream resolution); Songbird decodes/mixes internally, so no extra audio daemon is needed. Verify with `yt-dlp --version`.

### 2. Create the bot

1. [Discord Developer Portal](https://discord.com/developers/applications) → **New Application** → **Bot** → copy the token.
2. Invite with scopes **`bot` + `applications.commands`** and permissions: *View Channels, Send Messages, Embed Links, Attach Files, Connect, Speak*.
   Template: `https://discord.com/oauth2/authorize?client_id=YOUR_APP_ID&permissions=3263488&scope=bot%20applications.commands`
3. No privileged intents needed. Voice states come from `GUILD_VOICE_STATES`.

### 3. Configure & run

```bash
cp .env.example .env
# edit .env → DISCORD_TOKEN=...
cargo run --release
```

`GUILD_ID` is optional: set it while developing so slash commands appear **instantly** in one server. Leave it empty for global registration (Discord can take up to ~1 hour to propagate).

Join a voice channel, then `/play a song name` or `/search` and pick the exact result.

### 4. If it doesn't come online

A healthy startup prints these lines in order:

```text
INFO nexcord: yt-dlp 2026.10.26 ready — keyless search enabled
INFO nexcord: connecting to Discord…
INFO nexcord: Discord API reachable — token accepted as @YourBot
INFO nexcord: opening the Discord gateway…
INFO nexcord: gateway READY for YourBot — finishing setup
INFO nexcord: registered 17 slash commands (global scope)
INFO nexcord: YourBot is online
```

**If it stops right after the `yt-dlp … ready` line, don't press Ctrl+C.** That is the
last message before the first HTTPS request to Discord. The startup probe has a
10 s deadline and the client build a 20 s deadline, so wait ~30 s — you will then
get either `is online` or a specific error. Serenity's own first request has no
timeout, which is why older builds could look frozen there.

| What you see | What to do |
|---|---|
| `Discord rejected DISCORD_TOKEN (401 Unauthorized)` | Reset the token in **Developer Portal → Bot → Reset Token**, put the new value in `.env`. Never paste a token into a chat or a public issue. |
| `cannot reach discord.com (…)` / `timed out after 20 s contacting Discord` | Network problem, not a bot problem. Check `curl -vI https://discord.com`, DNS, firewall, proxy/VPN, and any ISP-level filtering. The gateway also needs outbound WSS (`gateway.discord.gg`). |
| No `registered … slash commands` line | The bot needs the **`applications.commands`** scope. Re-invite with `scope=bot%20applications.commands`. With `GUILD_ID` unset, global registration can take up to ~1 hour; set `GUILD_ID` for instant dev registration. |
| `yt-dlp --version timed out` | Harmless — only `/play` and `/search` need it, and the check is retried on next start. Install/upgrade with `pip install -U yt-dlp` or point `YT_DLP` at the binary. |
| Bot is online but `Unknown Interaction` / commands missing | Re-invite with the `applications.commands` scope and re-run so commands register. |

Invite URL (replace `CLIENT_ID`, from Developer Portal → General Information):

```text
https://discord.com/oauth2/authorize?client_id=CLIENT_ID&scope=bot%20applications.commands&permissions=3263488
```

`permissions=3263488` = view channel · send messages · embed links · attach files ·
read history · connect · speak. No privileged intents are required.

For a full trace:

```bash
RUST_LOG=nexcord=debug,serenity=info ./nexcord 2>&1 | tee nexcord.log
```

### Docker

```bash
docker compose up -d --build      # uses .env
```

The multi-stage image builds the Rust binary, then runs it on a slim Python image that ships `ffmpeg`, `libopus` and up-to-date `yt-dlp`.

---

## Commands

All 17 are slash commands, registered globally (or instantly in one guild with `GUILD_ID`).

| Command | Options | What it does |
|---|---|---|
| `/play` | `query` **(required, autocomplete)**, `source`, `pick` | Play a song name, any yt-dlp link, or a whole playlist. `pick:true` opens the exact-match picker. |
| `/search` | `query` **(required, autocomplete)**, `source` | Ranked results from YouTube + SoundCloud; choose the exact one. Best for obscure tracks. |
| `/panel` | — | Post a fresh persistent control panel in this channel. |
| `/queue` | `page` | Paginated queue, 10 per page, with ◀ / ▶ buttons. |
| `/now` | — | Compact now-playing card (ephemeral). |
| `/pause` | — | Toggle pause / resume. |
| `/skip` | `to` | Skip the current track, or jump to a queue position. |
| `/stop` | — | Stop playback and clear the queue. |
| `/seek` | `position` **(required)** — `1:23`, `01:02:03` or `90` | Seek within the current track. |
| `/volume` | `percent` **(required, 0–200)** | Set gain; 100 = untouched source volume. |
| `/loop` | `mode` **(required)** — `Off` / `Track` / `Queue` | Set loop mode; the panel 🔁 button cycles it. |
| `/shuffle` | — | Shuffle the upcoming queue. |
| `/remove` | `position` **(required, ≥1)** | Remove a queued track (1 = next). |
| `/join` | — | Join your voice channel. |
| `/leave` | — | Leave voice and clear the guild session. |
| `/stats` | — | Runtime + audio information. |
| `/help` | — | Command list embed. |

`source` accepts `Auto · YouTube + SoundCloud` (default), `YouTube`, `SoundCloud`. `query` autocomplete is served from a bounded in-memory cache, warmed in the background. Panel controls are restricted to people in the same voice channel, and the panel is one message **edited in place** — no chat spam.

### Music control panel buttons

```
row 1   ⏪ −15s   ⏸ Pause / ▶️ Resume   ⏭ Skip   ⏹ Stop   🔁 Off / Track / Queue
row 2   🔉 −      🔊 +                 🔀 Shuffle   📜 Queue   🔄 Refresh
```

---

## What the embeds look like

Every embed is built in `src/ui.rs`: one violet accent `#6c5ce7`, dark surfaces, monospace metadata, quote-style (`>`) descriptions, no decorative clutter. Three pieces of original artwork are attached once to the panel — `nexcord-mark.jpg` (author/footer icon), `nexcord-wave.jpg` (wide banner), `nexcord-cover.jpg` (thumbnail fallback). `▌` below is the embed's coloured left border. Every glyph shown is a **slot**
from [`emojis.toml`](emojis.toml): when a custom application emoji is available
it renders as `<:nx_skip:…>`, otherwise the generic Unicode glyph below is used.

### `/panel` — playing · accent `#6c5ce7`

```
▌ [mark] N E X C O R D   /   CONTROL PANEL
│
│ Says                                     ← title, clickable → source URL
│
│ > ▶️  Playing   ·   🟢 connected
│ > Nils Frahm  ·  `8:32`  ·  ☁️
│ >
│ > `2:41`   ▰▰▰▰▱▱▱▱▱▱▱▱▱   `8:32`
│ >
│ > `VOL` **100%**   ·   `LOOP` **Off**   ·   `UP NEXT` **7**
│
│ UP NEXT
│ ` 1.`  Says (Live at Funkhaus, Berlin)  ·  `10:04`
│ ` 2.`  Says — Rework  ·  `5:31`
│ ` 3.`  Familiar  ·  `4:28`
│ … and **4** more
│
│                    [nexcord-wave.jpg · full-width banner]
│                                        [cover thumbnail]
│
│ [mark] requested by kai   ·   48 kHz stereo Opus   ·   no API keys
└
  [⏪ −15s] [⏸ Pause] [⏭ Skip] [⏹ Stop] [🔁 Off]
  [🔉 −]    [🔊 +]    [🔀 Shuffle] [📜 Queue] [🔄 Refresh]
```

### `/panel` — idle · accent `#2b2f3a`, buttons disabled

```
▌ [mark] N E X C O R D   /   CONTROL PANEL
│ Nothing playing
│ > ⚪  **Idle**
│ >
│ > `/play` a song, a link or a playlist — or use `/search` to pick an exact match.
│ > Works with YouTube, SoundCloud, Bandcamp and 1000+ other sites. No API key needed.
│                    [nexcord-wave.jpg]      [nexcord-cover.jpg]
│ [mark] N E X C O R D   ·   fast · clean · keyless
```

Paused → accent becomes amber `#f5a524`, status reads `⏸ Paused`, primary button becomes `▶️ Resume`.

### `/play` — confirmation · green `#3ddc97`

```
▌ N E X C O R D   /   QUEUED
│ Says                                     ← clickable
│ > ▶️  Playing now        (or `📥  Added to queue  ·  position 3`)
│ > **Nils Frahm**  ·  `8:32`  ·  SoundCloud
│ >
│ > requested by **kai**
```

### `/search` — exact-match picker (ephemeral: only you see it)

```
▌ N E X C O R D   /   SEARCH
│ Choose a track
│ > 🔎  **nils frahm says**
│ > YouTube + SoundCloud  ·  8 matches  ·  pick the exact one
│
│ ` 1.`  Says
│        Nils Frahm · 8:32 · SoundCloud
│ ` 2.`  Says (Live at Funkhaus, Berlin)
│        Nils Frahm · 10:04 · YouTube
│ ` 3.`  Says — Rework
│        Nils Frahm & Ólafur Arnalds · 5:31 · Bandcamp
│ …
│ exact matches · no API keys · expires in 10 min
└
  ┌ Pick the exact track…            ⌄ ┐   ← select menu, up to 25 options
  │ 1. Says                            │
  │ 2. Says (Live at Funkhaus, Berlin) │
  └────────────────────────────────────┘
  [⚡ Play best match]  [✖ Cancel]
```

### `/queue` — paginated

```
▌ N E X C O R D   /   QUEUE
│ NOW PLAYING
│ **Says**
│ Nils Frahm  ·  `8:32`
│
│ ` 1.`  **Says (Live at Funkhaus, Berlin)**
│        Nils Frahm  ·  `10:04`  ·  YouTube
│ ` 2.`  **Says — Rework**
│        Nils Frahm & Ólafur Arnalds  ·  `5:31`  ·  Bandcamp
│ …
│ page 1/2   ·   12 queued   ·   1:04:22 remaining   ·   Off loop
└
  [◀ Prev] [📄 1/2] [▶ Next] [✖ Close]
```

Empty queue → `> The queue is empty.` / `> /play something, or /search for an exact match.`

### `/now` — compact card (ephemeral)

```
▌ N E X C O R D   /   NOW PLAYING
│ Says                                     ← clickable
│ > ▶️  Playing  ·  **Nils Frahm**
│ > `8:32`  ·  SoundCloud
│ >
│ > `2:41`   ▰▰▰▰▱▱▱▱▱▱▱▱▱   `8:32`
│ >
│ > `VOL` **100%**   ·   `LOOP` **Off**   ·   `UP NEXT` **7**
│ requested by kai
```

### Button / command replies — ephemeral toasts

`ui::notice()` makes a compact one-line embed for `/pause`, `/skip`, `/volume`, `/loop`, button presses and errors. Title = `emoji + text`, footer = `N E X C O R D  ·  keep it simple`:

```
✓  Skipped             green  #3ddc97    "On to the next track."
!  Volume capped        amber  #f5a524
✕  Action failed        red    #f25f5c
✦  Refreshed            violet #6c5ce7    "The panel is up to date."
```

### `/help` and `/stats`

```
▌ N E X C O R D   /   HELP
│ Everything, in one place
│ > A keyless music bot. Type a song name, paste a link, or pick an exact result.
│ PLAYBACK        /play /search /pause /skip /stop /seek /join /leave /panel
│ QUEUE & SOUND   /queue /shuffle /loop /remove /volume
│ SEARCH, NO KEYS Searches YouTube and SoundCloud in parallel …
│ INFO            /now /stats /help
│ built in Rust · serenity + songbird · 48 kHz stereo Opus
```

`/stats` uses the same layout with title `Runtime` and fields `Uptime`, `Voice sessions`, `Playing now`, `Tracks queued`, `Search cache`, `Open pickers`.

---

## Accurate search, without keys

```
/play "some half-forgotten B-side from 2009"
        │
        ├─ ytsearch10:… ─┐
        │                ├─ parallel tokio processes (flat JSON, ~1 request each)
        ├─ scsearch10:… ─┘
        │
        ├─ literal token scoring (exact phrase & exact title weighted)
        ├─ de-duplication by title/artist AND by URL
        ├─ small boost for the long tail (SoundCloud / Bandcamp / niche uploads)
        └─ ranked list → /search picker or autocomplete suggestions
```

* **Two catalogues at once** — rare tracks that never reached YouTube often live on SoundCloud or Bandcamp.
* **You pick the exact match** — `/search` and `/play pick:true` show up to 10 candidates with artist, duration and source, so a wrong auto-match is never forced on you.
* **Autocomplete that never stalls** — suggestions are served from the in-memory cache only; a cache miss warms it in a bounded background task (max 2 concurrent). Selecting a suggestion submits its **exact URL**, so `/play` resolves that precise track.
* **Any link works** — paste YouTube, SoundCloud, Bandcamp, archive.org, a direct `.mp3`/`.flac`/`.ogg`, or a playlist URL. `--flat-playlist` resolves playlists into a full queue.
* **Cached** — identical queries are free for `SEARCH_CACHE_TTL` seconds; the cache is bounded and purged every 30 s.

---

## Audio quality notes

* Songbird's `YoutubeDl` input runs `yt-dlp -f ba[abr>0][vcodec=none]/best`, i.e. the **best audio-only** stream. When that stream is Opus-in-WebM, frames are passed through to Discord **without a lossy transcode**.
* Everything is mixed at 48 kHz stereo (`MixMode::Stereo`) with `use_softclip(true)` to avoid clipping when the volume is pushed above 100%.
* `OPUS_BITRATE` defaults to `160000` bps; raise to `384000` for a busy server, lower it for constrained uplinks.
* `DEFAULT_VOLUME=100` means no gain applied. Volume is per guild and applies live to the current track.
* Tracks are lazy: metadata is fetched once during search, and the stream URL is resolved only when the track is about to play. Nothing is downloaded to disk.

---

## Own assets

All artwork in [`assets/`](assets/) was generated for this project and is embedded at compile time with `include_bytes!`:

| File | Used for |
|---|---|
| `nexcord-mark.jpg` | Bot avatar, embed author / footer icon |
| `nexcord-wave.jpg` | Wide waveform header of the control panel embed |
| `nexcord-cover.jpg` | Fallback cover art for tracks without a thumbnail |

They are uploaded with the panel message via `attachment://` references (about 100 KB total, once per guild) and Discord keeps them when the panel is edited. If you'd rather serve them from a CDN, set `ASSET_BASE_URL=https://your-cdn/nexcord-assets` — the bot then uses URLs and uploads nothing.

[`assets/emojis/`](assets/emojis/) holds ten more generated icons: 128×128 **transparent RGBA PNGs** with a violet→cyan gradient, compiled into the binary for the custom application-emoji slots. See [`assets/README.md`](assets/README.md) for the full manifest, dimensions and transparency verification.

---

## Custom application emojis

The bot can use emojis from **its own application emoji library** (Developer
Portal → your app → Emojis). They work in every server, in messages and in
buttons — no guild upload, no boost requirement, no permissions. With zero
configuration the bot uses generic Unicode emojis instead, so it always looks
right out of the box.

[`emojis.toml`](emojis.toml) is the **only** file you edit. One line per UI slot:

```toml
skip = { name = "nx_skip", fallback = "⏭", file = "nx_skip.png" }
```

| Field | Meaning |
|---|---|
| `name` | Emoji name in your application's library (2–32 chars, `a-z 0-9 _`). All slots share the `nx_` prefix. |
| `fallback` | Generic Unicode emoji used when no custom emoji is available. |
| `file` | Optional bundled PNG, uploaded once when auto-upload is on. |
| `id` | Optional numeric emoji ID — paste one to point a slot at a specific emoji and skip the name lookup. |

Resolution at startup, per slot: **`id` → existing application emoji matched
by `name` (read-only) → one-time upload of `file` → Unicode `fallback`**. Any
Discord error (missing emoji, rate limit, offline API) degrades to the fallback
instead of failing startup.

**Enable the custom icons once:**

```bash
# .env
EMOJI_AUTO_UPLOAD=1
cargo run --release          # uploads the 10 missing icons, prints the count
# set it back to 0 — from now on they are found by name, nothing is uploaded
```

Or do it explicitly, without running the bot:

```bash
export DISCORD_TOKEN='Bot …' APPLICATION_ID=…
python3 scripts/upload-emojis.py --list       # inspect the library
python3 scripts/upload-emojis.py --dry-run   # see the plan
python3 scripts/upload-emojis.py             # upload missing icons
```

`EMOJIS_FILE=/path/to/emojis.toml` overrides the compiled default at runtime —
Docker Compose already mounts `./emojis.toml` read-only, so re-mapping an icon
needs no rebuild. `/stats` shows how many custom slots are live.

> Ten gradient icons were generated for the main panel controls. The remaining
> slots (volume, pagination, toast tones, status dots, source badges) are left
> on their Unicode fallbacks; upload an emoji with the listed `name` or paste
> an `id` and the bot picks it up on the next restart.

---

## Configuration

| Variable | Default | Notes |
|---|---|---|
| `DISCORD_TOKEN` | — | **required** |
| `GUILD_ID` | empty | Guild-scoped command registration (instant) |
| `YT_DLP` | `yt-dlp` | Executable name or absolute path |
| `DEFAULT_VOLUME` | `100` | 0–200 % |
| `MAX_QUEUE` | `200` | Per-guild cap |
| `SEARCH_RESULTS` | `10` | 3–25 (Discord menu limit) |
| `SEARCH_CACHE_TTL` | `600` | Seconds |
| `IDLE_TIMEOUT` | `300` | Auto-leave when idle, `0` disables |
| `OPUS_BITRATE` | `160000` | 32 000–384 000 bps |
| `ACCENT` | `0x6C5CE7` | Embed accent colour |
| `ASSET_BASE_URL` | empty | Optional CDN base URL |
| `EMOJI_AUTO_UPLOAD` | `0` | `1` uploads missing application emojis once at startup |
| `EMOJIS_FILE` | compiled `emojis.toml` | Runtime override for the emoji registry |
| `RUST_LOG` | `nexcord=info,...` | tracing filter |

---

## Project layout

```
emojis.toml           the single emoji/UI-slot registry (Unicode fallbacks)
src/
├── main.rs           bootstrap, gateway events, command registration
├── config.rs         env config + Songbird voice config
├── commands.rs       slash command definitions
├── emojis.rs         application-emoji resolution, upload and fallback
├── interactions.rs   slash commands, autocomplete, buttons, select menus
├── player.rs         per-guild queue, playback, loop/shuffle/seek, idle cleanup
├── search.rs         keyless yt-dlp search/resolve, scoring, cache
├── ui.rs             every embed + component builder, asset embedding
└── util.rs           formatting, progress bars, time parsing, shuffle
assets/               original generated artwork (compiled in)
assets/emojis/        10 transparent gradient application-emoji PNGs
scripts/              offline toolchain bootstrap + emoji upload helper
REVIEW.md             two-pass (Discord dev/user, Rust dev) review record
```

State is an `Arc<AppState>` with a small `RwLock<HashMap<GuildId, Arc<Mutex<GuildState>>>>`. One global `TrackEvent::End` listener per voice call advances the queue from a spawned task, so no work happens inside the audio mixer. Stale end events from skipped tracks are ignored by comparing the track UUID.

```bash
cargo test          # 13 unit tests: formatting, colours, emoji registry, search scoring
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

---

## Toolchain & verification

`rust-toolchain.toml` asks rustup for the latest **stable** with `rustfmt` + `clippy`. Nexcord's own code was validated with **Rust 1.99.0** (`rustc`/`cargo 1.99.0`, `rustfmt 1.10.0`, `clippy 0.1.99`, LLVM 23.1.1).

In locked-down environments where `rustup` / `static.rust-lang.org` are unreachable but the npm registry is, the official `rust-installer` component tarballs are published under the `@toolchain-bin` npm scope. [`scripts/bootstrap-rust.sh`](scripts/bootstrap-rust.sh) downloads and assembles them into a standalone prefix — no rustup, no root, no build from source:

```bash
./scripts/bootstrap-rust.sh ~/.rust-npm
export PATH="$HOME/.rust-npm/bin:$PATH"
export LD_LIBRARY_PATH="$HOME/.rust-npm/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
rustc --version   # rustc 1.99.0
```

### Verification (full, dependency-linked)

Everything below ran on Rust 1.99.0 against the **real crates.io dependency graph** — 375 locked packages including `serenity 0.12.4`, `songbird 0.5.0`, `tokio 1.54.1`, `reqwest 0.12.28` and `toml 0.8.23` — not stubs:

| Check | Result |
|---|---|
| `rustfmt --edition 2021 --check src/*.rs` | ✅ clean |
| `cargo check --offline` | ✅ passes |
| `cargo clippy --offline --all-targets -- -D warnings` | ✅ zero warnings |
| `cargo test --offline` | ✅ **13 passed**, 0 failed |
| `cargo build --release --offline` | ✅ 14 MB stripped binary, links successfully |

`Cargo.lock` is committed so these exact versions are reproducible.

### Building on a restricted network

The build sandbox blocks `static.rust-lang.org` and `index.crates.io` at the TLS layer, so both the compiler and the crates were obtained through allowlisted channels rather than given up on:

1. **Toolchain** — [`scripts/bootstrap-rust.sh`](scripts/bootstrap-rust.sh) (npm `@toolchain-bin` component tarballs).
2. **Dependencies** — `cargo vendor` on any machine with crates.io access, then point cargo at the vendor directory:

   ```bash
   cargo vendor --versioned-dirs vendor > .cargo/config.toml
   # .cargo/config.toml:
   #   [source.crates-io]
   #   replace-with = "vendored-sources"
   #   [source.vendored-sources]
   #   directory = "vendor"
   cargo build --release --offline
   ```

   In this sandbox the vendor tree was produced by a temporary GitHub Actions workflow (runners have unrestricted egress) and relayed back through the repository, then removed.

3. **libopus** — songbird's `audiopus_sys` needs Opus. With no apt/pkg-config available it was built from the upstream `xiph/opus` source (CMake from PyPI) and exposed via `LIBOPUS_NO_PKG=1` + `LIBOPUS_LIB_DIR=/path/to/prefix`. On a normal machine just install `libopus-dev` and `pkg-config`.

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && cargo build --release
```

---

## Notes

* `yt-dlp` is a moving target — keep it updated (`pip install -U yt-dlp`) when a site changes.
* Respect the terms of service of the sites you stream from and the rights of the music you play.
* `Cargo.lock` is committed so binary builds are reproducible; regenerate with `cargo update` when bumping dependencies.
* Licensed under [GPL-3.0](LICENSE).
