# Two-pass engineering review

Every file was revised twice, in two distinct roles:

- **Pass 1 — Discord developer + end user.** Is this correct against the Discord
  API (limits, component shapes, emoji ownership, rate limits, permissions), and
  is it pleasant/obvious for someone using the bot?
- **Pass 2 — Rust developer.** Is it type-safe, panic-free, allocation-conscious,
  idiomatic, clippy-clean and covered by tests?

Findings were applied, not just listed. The full suite was re-run afterwards
(`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`,
`cargo build --release`).

| File | Pass 1 · Discord dev + user | Pass 2 · Rust dev |
|---|---|---|
| `emojis.toml` *(new)* | One line per UI slot; uses the **application's own emoji library** (works in every guild, no boost/permissions), generic Unicode fallback so zero setup still looks right; `nx_` namespace, aliases (`play`/`playing`) documented; `EMOJIS_FILE` override so ops can edit without a rebuild. | Typed `serde` deserialisation; startup validation of required slots, name syntax (2–32 alnum/underscore), prefix and non-empty fallback; optional `id` for explicit overrides; bad file fails loudly instead of silently rendering ✦. |
| `src/emojis.rs` *(new)* | Resolution order `id → library-by-name → one-time upload → Unicode`; uploads deduplicated by name; `<:name:id>` for embed text and `ReactionType::Custom` for buttons; any API/rate-limit error degrades to Unicode; reconnect guard avoids re-uploading. | `OnceLock<RwLock<..>>` process-wide immutable registry — cheap reads, no lifetime plumbing through ~40 UI call sites; poison recovery; no `unwrap` outside tests; PNGs via `include_bytes!`; 5 unit tests incl. malformed config. |
| `src/main.rs` | Resolve emojis only after `set_application_id`, once per process; startup log tells the operator how many custom slots are live. | `mod emojis`; `EmojiMap::load()?` fails fast on a broken TOML; `resolved()`/`mark_resolved()` guard gateway reconnects. |
| `src/config.rs` | `EMOJI_AUTO_UPLOAD` documented as an explicit opt-in (no surprise writes to the app on first boot). | Invalid numeric env values now `warn!` instead of silently taking the default; `num` bounded by `Display + Copy`; accent-colour parsing covered by a test; test module moved to EOF for `clippy::items_after_test_module`. |
| `src/commands.rs` | `/queue page` and `/skip to` now reject `0`/negatives with `min_int_value(1)`; option descriptions stay under 100 chars; choice lists ≤ 25. | Builder parentheses fixed after adding the bounds; rustfmt-clean. |
| `src/ui.rs` | **No hard-coded emoji anywhere** — every button, toast, status and source badge goes through a registry slot; `custom_id` and select-menu values capped at Discord's 100-byte limit; embed descriptions use named placeholders so the 4096-char limit stays auditable. | `format!`-inside-`format!` removed (clippy `format_in_format_args`); named args for the 10-placeholder panel/now cards; `util::clamp_id` cuts at a UTF-8 boundary; no `ReactionType` construction left in the UI layer. |
| `src/interactions.rs` | Ephemeral toasts, voice-channel permission guard, picker expiry and immediate deferral all preserved; custom-id routing unchanged. | `trim_start_matches("nx:top:")` → `strip_prefix` — the former strips *repeated* prefixes and would corrupt a crafted id; no `unwrap` in production paths. |
| `src/player.rs` | Bursts of button presses can hit the message-edit rate limit: concurrent refreshes are now coalesced, the newest state wins; `/stats` reports live custom-emoji count. | `panel_gen` monotonic stamp checked before the network call; no `MutexGuard` held across `.await`; `wrapping_add` avoids overflow; field surfaced in `Snapshot` correctly. |
| `src/search.rs` | Source badges now resolve through the same emoji registry, so a custom icon appears consistently in every embed. | `Source::emoji()` → `emoji_key()`: the data layer returns a lookup key, presentation stays in `ui.rs` (separation of concerns, no emoji literals outside the config). |
| `src/util.rs` | Discord counts `custom_id` in **bytes** — added `clamp_id` so a long session id can never emit an invalid component. | `truncate(s, 0)` no longer returns a 1-char string; truncation remains char-based (Discord embed limits are characters); 4 unit tests. |
| `Cargo.toml` / `Cargo.lock` | No command framework, no extra runtime service; new deps are config-parse only. | `serde` (derive), `toml 0.8`, `base64 0.22` (already in the graph); `Cargo.lock` regenerated and committed; `rust-version = "1.82"` still holds. |
| `Dockerfile` | `emojis.toml` is `include_str!`-ed, so it **must** be copied at build time; emoji PNGs are `include_bytes!`-ed — the runtime image still needs no asset files. | `COPY Cargo.lock` added for reproducible builds; inline comment moved to its own line (Dockerfile does not support trailing comments). |
| `docker-compose.yml` | Read-only `./emojis.toml` mount + `EMOJIS_FILE` so operators can re-map an icon without rebuilding. | Valid YAML, no secrets in the file; `env_file` retained. |
| `.env.example` | Documents `EMOJI_AUTO_UPLOAD`, `EMOJIS_FILE` and the exact zero-setup → custom-emoji upgrade path. | Boolean values listed (`1/true/yes/on`). |
| `.gitignore` / `.dockerignore` | Stale `preview` entry removed; `emojis.local.toml` ignored for local experiments. | Emoji PNGs deliberately **not** ignored (they are compile-time inputs). |
| `assets/emojis/*.png` *(new)* | 128×128 RGBA, transparent corners, violet→cyan gradient, each under 256 KB; 10 slots, remaining slots honestly left on Unicode fallback. | Compiled into the binary with `include_bytes!`; total ~117 KB, measured and verified; documented in `assets/README.md`. |
| `scripts/upload-emojis.py` *(new)* | Idempotent by name; `--list`, `--dry-run`; prints IDs; never duplicates an existing emoji. | Stdlib only (`tomllib`, `urllib`); 256 KB guard, HTTP error bodies surfaced, no third-party dependency. |
| `scripts/bootstrap-rust.sh` | Reproducible toolchain install on locked-down networks. | SHA-256 pinning of all 5 component tarballs (a swapped tarball can no longer become the compiler), curl retries/timeouts, fail-loud version check, `mktemp` trap cleanup. |
| `assets/README.md` *(new)* | Records exactly which assets were generated, their sizes, transparency and which emoji slot each serves. | Notes the `include_bytes!`/`include_str!` build-time coupling. |
| `README.md` | Commands, embed examples and the new custom-emoji workflow documented; the interactive UI mock was removed on request. | Verification table, configuration and project layout kept in sync with the code. |
| `preview/`, `index.html` | **Removed** — a dashboard/mock was not requested; the bot is docs + code only. | All references (README, `.dockerignore`, root redirect) removed; no live server left running. |
| `rust-toolchain.toml` / `LICENSE` | No user-visible change. | `stable` + `rustfmt` + `clippy` confirmed against Rust 1.99.0; GPL-3.0 retained for code and assets. |

## Verification after both passes

| Check | Result |
|---|---|
| `cargo fmt --check` | ✅ clean |
| `cargo clippy --offline --all-targets -- -D warnings` | ✅ zero warnings |
| `cargo test --offline` | ✅ **13 passed, 0 failed** (7 new tests) |
| `cargo build --release --offline` | ✅ links against real `libopus` |
| Emoji PNG alpha | ✅ 10/10 RGBA, transparent corners, 128×128 |

All checks run against the real 375-crate dependency graph, not stubs.
