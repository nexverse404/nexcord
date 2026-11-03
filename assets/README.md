# Assets

Every file here was **generated for Nexcord with an image-generation tool** and
post-processed locally. No stock art, no third-party icons, no external URLs at
runtime unless `ASSET_BASE_URL` is set.

## Brand artwork (compiled into the binary)

| File | Format / size | Used as |
|---|---|---|
| `nexcord-mark.jpg` | JPEG · 512×512 · 27.2 KB | author + footer icon on the control panel, bot avatar |
| `nexcord-wave.jpg` | JPEG · 1200×396 · 36.5 KB | full-width banner on the **idle** control panel only |
| `nexcord-cover.jpg` | JPEG · 512×512 · 36.7 KB | thumbnail on the **idle** control panel; a playing track shows its own remote album art instead |

Total **108 KB**. Referenced from `src/ui.rs` via `include_bytes!`, uploaded once
with the panel message as `attachment://…`, or served from a CDN when
`ASSET_BASE_URL` is configured.

## Custom application emojis (`assets/emojis/`)

Ten original gradient icons, **violet `#7b6cf6` → cyan `#35d6e8`**, drawn as
single flat vector glyphs. Each is a **128×128 RGBA PNG with a fully transparent
background** — verified: corner alpha = 0, no white/grey halo. They are compiled
into the binary so uploads also work from the minimal Docker image.

| File | Size | Slot in `emojis.toml` | Generic fallback |
|---|---|---|---|
| `nx_play.png` | 11.2 KB | `play`, `playing` | ▶️ |
| `nx_pause.png` | 8.8 KB | `pause`, `paused` | ⏸ |
| `nx_back.png` | 9.1 KB | `back` | ⏪ |
| `nx_skip.png` | 12.2 KB | `skip` | ⏭ |
| `nx_stop.png` | 16.2 KB | `stop` | ⏹ |
| `nx_loop.png` | 11.2 KB | `loop` | 🔁 |
| `nx_shuffle.png` | 14.1 KB | `shuffle` | 🔀 |
| `nx_queue.png` | 10.2 KB | `queue`, `added` | 📜 / 📥 |
| `nx_refresh.png` | 13.2 KB | `refresh` | 🔄 |
| `nx_search.png` | 11.1 KB | `search` | 🔎 |

Total **~117 KB**, all well under Discord's 256 KB emoji limit. The generated
images originally had solid backgrounds; a local chroma-key pass removed the
neutral background, un-blended the antialiased edges, cropped to the glyph and
downsampled to 128×128 with Lanczos.

### Slots without a custom icon

`volume_down`, `volume_up`, `prev`, `next`, `page`, `close`, `cancel`, `top`,
the four toast tones, `connected` / `offline` / `idle` and the source badges use
their **Unicode fallbacks** from `emojis.toml`. They are still real slots: upload
an emoji with the listed `name` (or paste its `id`) and the bot picks it up on
the next restart — no code change. The image-generation cap for this session was
reached at 10 images, so these were deliberately left on fallback rather than
filled with lower-quality placeholders.

## Managing them

See [`emojis.toml`](../emojis.toml) for the slot map, and
[`scripts/upload-emojis.py`](../scripts/upload-emojis.py) for an explicit
upload/`--list`/`--dry-run` helper. With `EMOJI_AUTO_UPLOAD=1` the bot uploads
missing icons itself once at startup and then resolves them by name forever.

All assets are provided under the repository's [GPL-3.0](../LICENSE) license.
