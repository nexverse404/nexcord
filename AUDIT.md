# Line-by-line audit & revision

This pass was **documentation-driven**, not guess-driven. Every API change below
was checked against the actual released source/docs before being written.

## Sources actually read

| Source | Version / date | What it established |
|---|---|---|
| [Rust 2024 Edition Guide](https://doc.rust-lang.org/edition-guide/rust-2024/index.html) | 1.85 | `edition = "2024"` requires Rust ≥ 1.85; migration rules (reserved `gen`, `unsafe` attributes, `if let` temporary scope, match ergonomics). |
| [serenity `CommandInteraction`](https://docs.rs/serenity/0.12.5/serenity/model/application/struct.CommandInteraction.html) | 0.12.5, 2026‑10‑29 | fields `guild_id`, `channel_id`, `user`, `data`, `token`; `create_response` / `edit_response` / `create_followup` / `defer_ephemeral` are all present on command **and** component interactions. |
| [serenity `CreateInteractionResponse`](https://github.com/serenity-rs/serenity/tree/v0.12.5) | 0.12.5 source | `Defer` = "The user sees a loading state" (that is the "thinking…" message); `DeferredUpdateMessage` = "The user does **not** see a loading state". A deferral can only be followed by `edit_response`/`create_followup` — a second `create_response` is rejected. |
| [Poise crate docs](https://docs.rs/poise/latest/poise/) | 0.7.1, 2026‑10‑12 | `FrameworkOptions { on_error, commands, ..Default::default() }`, `FrameworkError`, `#[poise::command(slash_command)]`, `builtins::register_globally`, `serenity_prelude`; the framework guarantees a reply for every command. |
| [songbird releases](https://github.com/serenity-rs/songbird/releases) | v0.6.1, 2026‑10‑10 | v0.6.0 (2026‑04‑05) added **"support for Discord's Audio & Video End-to-end encryption (DAVE) protocol, which is since 2026 mandatory for all voice connections"**, switched `audiopus` → `opus2`, removed the `Option` around channel IDs. |
| [songbird v0.6.1 source](https://github.com/serenity-rs/songbird/tree/v0.6.1) | `src/driver/connection/mod.rs` | Identify sends `max_dave_protocol_version: Some(davey::DAVE_PROTOCOL_VERSION)`; `driver` feature pulls in `davey`. |
| [opus2 `Bitrate`](https://docs.rs/opus2/0.4.0/opus2/enum.Bitrate.html) | 0.4.0 | variants are `Bits(i32)`, `Max`, `Auto` — **not** `BitsPerSecond(i32)` as in `audiopus`. |
| [davey](https://docs.rs/davey/0.1.2/davey/) | 0.1.2 | pure-Rust MLS/DAVE implementation (`openmls`, `p256`), no C++ toolchain needed. |
| [Discord Opcodes & Status Codes](https://docs.discord.com/developers/topics/opcodes-and-status-codes) | — | voice close **4017 = "E2EE/DAVE protocol required"**; DAVE ops 21–31. |
| [Discord DAVE blog](https://discord.com/blog/bringing-dave-to-all-discord-platforms) | 2025‑09‑02 | from 2026‑03‑01 clients/apps without DAVE cannot join voice calls. |
| [Discord A/V E2EE enforcement](https://support.discord.com/hc/en-us/articles/38749827197591-A-V-E2EE-Enforcement-for-Non-Stage-Voice-Calls) | 2026‑03‑03 | out-of-date clients are rejected with 4017. |

### Framework decision

Poise 0.7.1 was evaluated as a replacement for the hand-written dispatcher.
It was **not** adopted, for documented reasons: Poise models prefix/slash
commands, but this bot's interface is 17 slash commands **plus** a persistent
message with 10 buttons, a paginated queue and a string-select picker. Poise has
no model for component interactions, so those would still need raw serenity
`Interaction` handling, giving two dispatch paths and an extra dependency —
against the project's stated *Lightness* goal. Instead, Poise's documented
guarantee ("`on_error` always produces a reply") is reproduced explicitly in
`dispatch`/`bounded`/`report_*_error`, and the registration flow follows
`builtins::register_globally`'s pattern.

## Root cause of the reported failures

1. **`/play` → "couldn't join voice: Driver(Ws(WsClosed(... 4017 ... "E2EE/DAVE protocol required")))"**
   `songbird 0.5.0` (pinned in `Cargo.toml`) predates DAVE. Its Identify sent
   `max_dave_protocol_version: 0`; Discord closed the voice WebSocket with 4017.
   This is a **dependency defect, not application logic**. Fixed by pinning
   `songbird = "=0.6.1"` (DAVE support via `davey`) and `serenity = "=0.12.5"`.
2. **"sent a private msg saying thinking"** — `defer_ephemeral()` emits
   `CreateInteractionResponse::Defer`, which per serenity's own source shows a
   loading state. Two paths could leave it unresolved:
   - `report_command_error` fell back to `create_response`, which Discord
     rejects once an interaction is acknowledged → the error was never shown;
   - `handle_pick` answered with "One moment…" and then detached a task whose
     result could be lost, leaving that card forever.
   Both are fixed: every interaction now has a 25 s deadline, error fallbacks
   use `create_followup`, and the picker does its work inline and always posts
   exactly one final followup.
3. **"could not play music"** — downstream of (1): the queue was filled but the
   voice connection never existed. Additionally, a lazy `YoutubeDl` input that
   fails to initialise emits `TrackEvent::Error` (songbird 0.6.1), which the old
   code did not listen for, so the queue could freeze with `current` set.
4. **Wiring bug found in the audit** — `YT_DLP` configured the search binary,
   but songbird's `YoutubeDl::new` always used its hard-coded `yt-dlp`. The
   configured program (and `YT_DLP_ARGS`) is now threaded into playback via
   `YoutubeDl::new_ytdl_like(...).user_args(...)`.

## Per-file findings and fixes

| File | Finding (line-level) | Fix |
|---|---|---|
| `Cargo.toml` | `edition = "2021"`; `serenity =0.12.4`, `songbird =0.5.0` (no DAVE). | `edition = "2024"`, `rust-version = "1.85"`, `serenity =0.12.5`, `songbird =0.6.1`, `[lints.rust] unsafe_code = "forbid"`, `version = 1.1.0`. |
| `Cargo.lock` | pinned to the pre-DAVE graph. | Regenerated on a real crates.io resolver; committed. |
| `rust-toolchain.toml` | asked for generic stable. | Kept; documented that edition 2024 needs ≥1.85 (verified on 1.99.0). |
| `src/main.rs` | `register_songbird_with(...).await` — the `.await` is on `ClientBuilder: IntoFuture`, **not** on `register_songbird_with` (verified in both v0.5.0 and v0.6.1 sources). Preflight had no log of the voice protocol. | No change to the builder chain (it was already correct); startup log now states the DAVE-capable songbird version; preflight/timeouts retained. |
| `src/config.rs` | `YT_DLP` had no way to pass cookies/PO tokens; `voice_config()` used the 0.5 API. | Added `yt_dlp_args: Vec<String>` from `YT_DLP_ARGS`; `voice_config()` verified against 0.6.1 (`mix_mode`, `use_softclip`, `preallocated_tracks` all still present). |
| `src/player.rs` | `Bitrate::BitsPerSecond` (audiopus) — does not exist in opus2. `current_channel().map(|c| c.0.get())` — 0.6.1 returns `Option<ChannelId>`. `join` failure left a stale `Call` in songbird's map so retries timed out. `TrackEndHandler::act` only used `list.last()` of a multi-track event. No `TrackEvent::Error` listener. | `Bitrate::Bits`, direct `Option<ChannelId>` comparison, `voice.remove()` cleanup + actionable error on join failure, iterate every pair in `EventContext::Track`, new `TrackErrorHandler` advancing the queue on source errors, `ytdl_program: &'static str` + `user_args` wired into `YoutubeDl`. |
| `src/interactions.rs` | `report_command_error` fallback used `create_response` after a defer (invalid). `handle_pick` detached a task after a "One moment…" card. `tracks.remove(0)` could panic on an empty vec. No deadline on any interaction. Picker session not checked against the clicking user. | `create_followup` fallback; inline picker with 20 s per-enqueue timeout and guaranteed final followup; `into_iter().next()` + explicit error; `INTERACTION_DEADLINE = 25 s` / `AUTOCOMPLETE_DEADLINE = 3 s`; `session.user_id == comp.user.id` check. |
| `src/search.rs` | `Searcher::new` had no way to pass extra yt-dlp flags; `spawn` built args directly. `check_available` deadline 8 s. | `extra: Vec<String>` appended **before** the URL (yt-dlp requires the target last), shared by search/resolve/playback; 15 s version deadline retained. |
| `src/ui.rs` | `search_components` could emit a `StringSelect` with an empty option list (Discord rejects it). | Returns only a Cancel button when `tracks.is_empty()`; all other builders re-checked against serenity 0.12.5 limits (`custom_id` 100 bytes, 5 buttons/row, 25 options, titles ≤256). |
| `src/commands.rs` | option names/descriptions and bounds. | Re-verified: 17 commands, all descriptions ≤100 chars, `min_int_value(1)` on `page`/`to`/`position`, `0–200` on `percent`, ≤25 choices. |
| `src/emojis.rs` | `get_application_emojis()` requires an application id. | Verified `ready` sets it before `resolve()`; `resolved()` guard prevents repeated API calls on reconnect; all 32 slots validated at startup. |
| `src/util.rs` | `truncate(s, 0)` previously returned 1 char; `custom_id` measured in bytes. | Fixed; `clamp_id` cuts at a UTF-8 boundary; tests retained. |
| `Dockerfile` | `rust:1.85-slim-bookworm` is the bare minimum for edition 2024. | `rust:1-slim-bookworm` (current stable) with a comment; `Cargo.lock`, `emojis.toml` and `assets/` all copied at build time for `include_str!`/`include_bytes!`. |
| `docker-compose.yml` | read-only `emojis.toml` mount. | Kept; `YT_DLP_ARGS` passes through `env_file`. |
| `.env.example` | no way to configure yt-dlp flags. | Added `YT_DLP_ARGS` with a cookies/PO-token example. |
| `README.md` / `REVIEW.md` | described 0.5.0 and 2021 edition; no DAVE note. | Updated: dependency table, DAVE requirement & 4017 troubleshooting, edition 2024, new verification results. |

## Wiring map (verified end to end)

```
main.rs
  config::load ─► Config (token, yt_dlp, yt_dlp_args, accent, opus_bitrate, …)
  emojis::EmojiMap::load ─► process-wide registry (32 slots, Unicode fallback)
  Http::new ─► check_discord_api (10 s) ─► Client::builder … register_songbird_with
                                              │
                     Songbird::serenity_from_config(config.voice_config())
                                              │
  AppState::new ─► Searcher (yt_dlp + args) ─┤
                 ├─ ytdl_program (&'static) ─┤
                 └─ voice: Arc<Songbird> ────┘
  EventHandler::ready ─► set_application_id ─► emojis::resolve ─► register commands
  EventHandler::interaction_create ─► tokio::spawn ─► interactions::dispatch
       ├─ Command ─► defer_ephemeral ─► handle_command ─► edit_response (always)
       ├─ Autocomplete ─► cached only, 3 s
       └─ Component ─► control buttons / picker / queue ─► one reply (always)
  player::AppState::enqueue ─► ensure_voice (songbird 0.6.1 DAVE join)
       └─ start_next ─► YoutubeDl::new_ytdl_like(config.yt_dlp) ─► Call::play_input
  TrackEvent::End / ::Error ─► spawned task ─► on_track_end ─► start_next
  refresh_panel ─► panel_gen coalescing ─► EditMessage / CreateMessage
```

## Verification

Run on the real dependency graph (not stubs) with Rust 1.99.0 and a locally
built `libopus`:

| Check | Result |
|---|---|
| `cargo fmt --check` | ✅ clean |
| `cargo clippy --all-targets -- -D warnings` | ✅ zero warnings |
| `cargo test` | ✅ all unit tests pass |
| `cargo build --release` | ✅ links against real `libopus` + `davey` |
| release binary startup | ✅ config → yt-dlp → Discord API preflight logs |

**Not verifiable in this sandbox:** a live Discord voice session. `discord.com`
egress is blocked here, so DAVE negotiation and actual audio output can only be
confirmed on the user's machine with a valid token and a joined voice channel.
