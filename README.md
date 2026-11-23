# FreeShow-rs

A native Rust rewrite of [FreeShow](https://freeshow.app/), the free presentation
app for churches and events. It opens an existing FreeShow library unchanged and
runs a Sunday service — songs, scripture, media, stage display and phone remote —
in a single ~30 MB process instead of an Electron app.

GPL-3.0-only. FreeShow 1.6.8 is the behavior reference; feature-by-feature status
is in [`docs/PARITY.md`](docs/PARITY.md).

| Measured on the dev box (`--check-perf`) | FreeShow-rs |
| --- | --- |
| Cold open | ~20 ms |
| Idle resident memory | ~30 MB |
| Go/Next to new audience frame | ~3 ms |
| Still-slide compositor | ~350 fps at 1080p |

## Architecture

```
            ┌────────────────────────── crates/freeshow (binary) ──────────────────────────┐
            │  Slint operator window ── Session (show, project, live state) ── live windows │
            └───────┬───────────────┬──────────────────┬──────────────────┬────────────────┘
                    │               │                  │                  │
          freeshow-data     freeshow-output      freeshow-media     freeshow-remote
          library files,    headless RGBA        stills, cpal +     axum + socketioxide
          importers,        compositor →         symphonia audio,   phone remote :5510
          scripture         wgpu upload          GStreamer video    stage page   :5511
                    │
          optional integrations (soft-fail when absent):
          osc · midi · timecode · ndi · record (RTMP/Icecast) · obs · pco · churchapps · cloud · ai
```

- **UI:** Slint 1.18 (winit + femtovg-wgpu) for the operator window. Audience and
  stage windows show frames painted by `freeshow-output` and uploaded with wgpu,
  so the preview, outputs, NDI and recordings are pixel-identical.
- **Data:** reads and writes FreeShow's own `Shows/`, `Config/`, `Bibles/`, `Media/`
  layout without dropping unknown JSON fields.
- **Integrations** are separate crates. A missing NDI plugin, encoder or account is
  logged and the show continues.

Details and the UI-stack decision: [`docs/MIGRATION_PLAN.md`](docs/MIGRATION_PLAN.md).

## Repository layout

```
crates/      16 workspace crates (binary: crates/freeshow)
docs/        usage, packaging, releasing, parity, plan
scripts/     packaging, CI helpers, perf bench, sample launcher
assets/      app icons and desktop entry
.github/     tag-only release workflows
```

| Crate | Purpose |
| --- | --- |
| `freeshow` | Operator window, session, output/stage windows, headless checks |
| `freeshow-core` | Theme colors, app settings file, logging |
| `freeshow-data` | Shows, projects, templates, overlays, stage layouts, scripture, importers |
| `freeshow-output` | Headless RGBA compositor (text, media, effects, transitions) |
| `freeshow-media` | Images, audio playback/routing, GStreamer video and cameras |
| `freeshow-remote` | Phone remote, stage web page, REST API, webhooks, Companion |
| `freeshow-osc` / `-midi` / `-timecode` | OSC, MIDI and MTC/LTC control input |
| `freeshow-ndi` / `-record` | NDI send/receive; recording, RTMP, Icecast, WHIP |
| `freeshow-obs` / `-pco` / `-churchapps` / `-cloud` / `-ai` | OBS WebSocket 5, Planning Center, ChurchApps, backup copy, local Ollama |

## Build and run

Requirements: Rust 1.97+ (`rust-toolchain.toml` selects stable) and GStreamer 1.x
development packages.

- **Ubuntu/Debian:** `libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev libgstreamer-plugins-bad1.0-dev libasound2-dev libudev-dev libfontconfig1-dev libxkbcommon-dev` plus the GStreamer good/bad/ugly/libav plugins.
- **macOS:** `brew install pkgconf gstreamer libnice-gstreamer` and `export PKG_CONFIG_PATH="$(brew --prefix)/lib/pkgconfig"`.
- **Windows:** the MSVC GStreamer SDK; `scripts/windows/setup-gstreamer.ps1` installs it.

```sh
cargo run -p freeshow                                    # empty library
cargo run -p freeshow -- --data ~/Documents/FreeShow     # existing FreeShow library
scripts/present-sample.sh                                # one-song demo on the output window
```

Headless checks (no window needed) — useful before every release:

```sh
cargo run --release -p freeshow -- --check-perf      # startup, memory, fps, Go/Next, remote latency
cargo run --release -p freeshow -- --check-service   # import → project → present → REST/Companion → stage
cargo run --release -p freeshow -- --check-library   # open a library, present, fingerprint frames
```

While running, a phone on the same network can open `http://<host>:5510/` (remote)
and `http://<host>:5511/` (stage).

## Development

Every commit must pass the local gate:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

GitHub Actions does **not** run on pushes or pull requests — only on `v*` tags (or
a manual run) to save CI minutes. See [`docs/RELEASING.md`](docs/RELEASING.md).

## Documentation

- [`docs/USAGE.md`](docs/USAGE.md) — features, library settings, integrations and environment variables
- [`docs/PACKAGING.md`](docs/PACKAGING.md) — installers, signing, diagnostics
- [`docs/RELEASING.md`](docs/RELEASING.md) — tag → GitHub release
- [`docs/PARITY.md`](docs/PARITY.md) — status against FreeShow 1.6.8
- [`docs/MIGRATION_PLAN.md`](docs/MIGRATION_PLAN.md) — architecture, budgets, milestones
- [`docs/ROADMAP.md`](docs/ROADMAP.md) — milestone history and working method
