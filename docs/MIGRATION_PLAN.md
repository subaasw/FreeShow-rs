# FreeShow-rs Architecture and Migration Plan

> Living plan for the native Rust rewrite of [FreeShow](https://freeshow.app/).
> Upstream behavior reference: ChurchApps/FreeShow **1.6.8** (this review used `1.6.8-beta.5`).
> Working status of individual features lives in [`PARITY.md`](../PARITY.md).
> Short-term working notes live in [`PLAN.md`](../PLAN.md).

**Goal:** A native, cross-platform presenter that opens an existing FreeShow library, edits a project, presents songs and scripture, runs a stage layout, plays media, and is controlled from the phone remote — with lower CPU, RAM, and startup cost than the Electron app, and without losing data written by that app.

**Architecture:** Keep the current crate split. Domain and file I/O stay headless and testable. A CPU compositor paints audience and stage frames to RGBA. The operator window is Slint; live windows upload compositor frames through wgpu. Optional integrations (NDI, record, MIDI, OSC, PCO, ChurchApps, OBS, AI) fail soft so a Sunday service still runs.

**Tech stack:** Rust 1.92+ (edition 2024), Slint 1.18 (winit + femtovg-wgpu), wgpu 30, GStreamer 1.x (video/cameras/NDI/RTMP), cpal + symphonia (audio files), axum + socketioxide (remote/stage), serde_json + thiserror, tokio (servers only), tracing.

## Global constraints

- License is GPL-3.0-only, matching upstream FreeShow.
- MSRV is `rust-version = "1.92"`. CI uses current stable with `rustfmt` and `clippy`.
- On-disk compatibility is mandatory: read and write existing FreeShow library files without dropping unknown JSON fields.
- Do not change the UI stack or crate layout unless a measured problem requires it.
- New behavior is tested in the crate that owns it. `cargo test --workspace` stays green.
- A show must not lose data or leak lyrics to the network without the FreeShow password handshake.
- Integrations are optional: a missing NDI plugin, encoder, or cloud account is logged and the show continues.

---

## 1. Current state (reviewed 2026-11-22)

The migration is far past a skeleton. `https://github.com/subaasw/FreeShow-rs` on `main` (`f7d53b3`) is a working native presenter with a 16-crate Cargo workspace, packaging scripts for macOS / Windows / Linux, and a detailed `PARITY.md` against FreeShow 1.6.8.

What already works end-to-end (the "service path"):

- Open a FreeShow data folder (`Shows/*.show`, `Config/projects.json`, templates, overlays, events, stage layouts, synced settings, `.fsb` Bibles).
- Present from the Slint operator window to audience and native stage windows, with fades, styles, stills, video, audio routing, timers, overlays, and QR boxes.
- Control the show from the phone remote (`:5510`) and view the stage page (`:5511`), including the FreeShow password handshake on Socket.IO, raw WebSocket, and HTTP.
- Import ProPresenter 4–7 (with important remaining graphic/timeline gaps), PowerPoint solid fills and embedded backgrounds, Lessons.church / ChurchApps media (when a download URL exists).
- Optional MIDI, OSC, NDI send, timecode, recording/RTMP/Icecast, Planning Center, OBS WebSocket 5, cloud zip backup, and local Ollama slide rewrite.
- Crash-safe saves (temp + rename, rotating `Backups`, 15-minute autosave) and relocatable installers (Linux tarball, Windows per-user installer, macOS .app/.dmg). `app` signing is ad-hoc; Developer ID / notarization is not done.

Two problems found on this review, both fixed in the first code commits of this plan:

1. `main` at `f7d53b3` did not compile. `feat(ui): stage layout` added `StageItemKind::Camera` and `device_name` / `device_type` but missed struct literals, the `"camera"` type map, and the stage paint match. Camera boxes now parse their saved device and show a label until a live feed is wired.
2. On GStreamer 1.26 (Debian 13 / similar), `audioconvert` rejects a `mix-matrix` whose row count does not already match the converter's output channel count. Speaker routing now waits for the new caps before applying the matrix. This is required for 3- and 4-channel cues on newer GStreamer; 1.24 (Ubuntu 24.04 CI) was more lenient.

### 1.1 Workspace layout (keep this)

```
crates/freeshow            # app shell: Slint operator, output/stage windows, session
crates/freeshow-core       # app settings, built-in themes, logging, speaker/network route types
crates/freeshow-data       # library I/O, .show JSON, imports, scripture, stage layouts, audio routing
crates/freeshow-output     # headless RGBA compositor (no window dependency)
crates/freeshow-media      # stills, cpal audio, GStreamer video/cameras, network audio buses
crates/freeshow-remote     # phone remote and stage HTTP/WebSocket/Socket.IO servers
crates/freeshow-osc        # OSC input
crates/freeshow-midi       # MIDI input
crates/freeshow-ndi        # NDI send of the audience picture
crates/freeshow-timecode   # MIDI timecode and LTC audio
crates/freeshow-record     # audience recording, RTMP, Icecast
crates/freeshow-pco        # Planning Center plans
crates/freeshow-churchapps # ChurchApps / Lessons.church
crates/freeshow-cloud      # library zip backup and optional cloud copy
crates/freeshow-obs        # OBS WebSocket 5
crates/freeshow-ai         # slide rewrite through local Ollama
```

Dependency direction stays downward: `freeshow` → integrations / `remote` / `media` → `output` + `data` → `core`. Integrations may depend on `data` or `media` but not on the app shell. This is how tests can paint a known slide or parse a `.show` file without opening a window.

### 1.2 Data compatibility

Show files are a JSON array `[id, show]`, which is what FreeShow writes. `freeshow-data` keeps unknown fields in the `serde_json::Value` so a save does not drop data this crate does not yet understand. That rule is load-bearing and must not be "cleaned up" into fully typed structs that drop unknown keys.

Library folders use the same names as Electron: `Shows/`, `Config/` (`projects.json`, `templates.json`, `overlays.json`, `events.json`, `stage.json`, `settings_synced.json`, `themes.json`), `Bibles/`, `Media/`, `Audio/`, `Imports/`, `Backups/`. The app also reads a subset of the library `settings.json` (theme, remote password, ports, transitions, audio routing). The rest of that file is still unread; new settings work should extend `FreeShowPreferences` field-by-field with tests against fixtures copied from upstream.

### 1.3 Build and quality baseline

- Workspace: Cargo resolver 3, edition 2024, `rust-version = "1.92"`, release uses thin LTO, one codegen unit, and symbol stripping.
- Tests: about 270 unit/integration tests plus a few ignored smoke tests (RTMP loopback, NDI SDK).
- Existing CI (`.github/workflows/{linux,windows,release}.yml`) is packaging-heavy: it compiles and runs tests, then builds and verifies installers. It does **not** run `rustfmt` or `clippy`, and macOS unit tests are not on the default pull-request path (they sit in the three-platform release workflow).
- `thiserror` is used in `freeshow-data`. Other crates still use ad-hoc error enums (`MediaError`, `SettingsError`). Standardize gradually; do not rewrite every crate at once.
- `tokio` is used where it belongs (the remote/stage servers). The operator session stays synchronous and timer-driven so it can live on the Slint thread. Do not "tokio-ify" the app shell without a reason.

---

## 2. UI stack: keep Slint + the headless compositor

The operator window is Slint 1.18 (`backend-winit`, `renderer-femtovg-wgpu`). Audience and stage windows are also Slint windows, but they do **not** draw slides as Slint widgets. `freeshow-output` paints RGBA frames; `freeshow` uploads those frames with wgpu (`unstable-wgpu-30`). That split is the most important design choice in the repo and it should stay.

### Why this is the right stack

| Option | Verdict |
| --- | --- |
| **Slint + wgpu compositor (current)** | Keep. Operator UI gets a native toolkit with a small runtime. Live output is a deterministic, testable painter that does not depend on CSS, a webview, or a GPU UI tree. Frame fingerprints are already used as tests. |
| Tauri (or "just a webview") | Reject. It would reintroduce a Chromium/WebView renderer for the thing we are leaving Electron to escape: lyric layout, fades, and output windows. Operator chrome could theoretically be HTML, but we would pay startup and RAM again and throw away the working Slint UI. |
| egui / iced / Dioxus | Reject as a rewrite. Immediate-mode or Rust-frontend stacks can work, but they would discard a large, already-themed operator window and would not paint lyrics more correctly than the existing compositor. |
| Pure wgpu / raw winit for the operator too | Too much UI work for menus, drawers, and editors. Slint is the right tool for chrome; wgpu is the right tool for the program output. |

**Rule:** do not churn working UI code to chase a different framework. Extract the inline `slint!` block out of `freeshow/src/main.rs` into `.slint` files when that file is next touched for a real feature, not as a standalone rewrite. `main.rs` (~2.4k lines) and `session.rs` (~6.4k lines) should be split along feature boundaries as those features are worked on.

---

## 3. Rust practices (adopt incrementally)

- **Errors:** `thiserror` for library crates, `tracing` for diagnostics. Reserve `anyhow` (if introduced) for the app shell only. Never drop errors in output or save paths.
- **Logging:** `tracing` + `tracing-subscriber` with `RUST_LOG` (already in place). Keep default `info`.
- **Serialization:** `serde` / `serde_json` for FreeShow JSON. Prefer `Value` plus typed views over a single huge fully-typed `Show` struct that can lose unknown fields.
- **Async:** tokio in `freeshow-remote` (and similar servers). GStreamer and cpal stay on their own threads. The Slint event loop is the UI thread.
- **Formatting:** `rustfmt` (edition 2024). `cargo fmt --all -- --check` in CI.
- **Lints:** `cargo clippy --workspace --all-targets --locked` in CI. Do **not** flip on `-D warnings` or `clippy::pedantic` in one shot; the tree already has dozens of default-allow or style warnings (`too_many_arguments` in the compositor, `bool_comparison` in tests). Tighten crate-by-crate: pedantic where a crate is small and clean (`freeshow-core` is a good first candidate).
- **CI:** a Quality workflow for fmt, clippy, and tests on Linux, Windows, and macOS, separate from the existing installer workflows. Cache cargo registries; do not package in the quality job.
- **Lockfile:** `--locked` in CI. Do not casually bump transitive deps during a feature commit.
- **Simplicity:** small conventional commits, no dead code, no speculative abstractions. Karpathy-style: the file that parses `.show` files should look like it parses `.show` files.

---

## 4. Performance targets

These are budgets, not current measurements. Add a lightweight bench or a `--check-perf` dry run before claiming them. Compare against the Electron app on the same machine and the same library.

| Budget | Target | Why it matters |
| --- | --- | --- |
| Cold start to operator window | ≤ 1.5 s on a mid-range laptop, mid-size library | Volunteers notice startup more than almost anything else. |
| Idle RSS (operator + output + stage, typical library, no video) | ≤ 150 MB | Electron commonly sits in the 400–800 MB range with two output windows. |
| Audience still + lyrics | 60 fps at 1920×1080 | One compositor frame plus a wgpu upload must fit in 16.6 ms. |
| Audience with 1080p video underlay | ≥ 30 fps, no audio underruns | GStreamer decode stays off the UI thread. |
| Go / Next to first output pixel (still slide) | ≤ 16 ms to start the fade | The operator's "did it go?" feeling. |
| Open a library of 1,000 shows | ≤ 500 ms to a usable show list | Scanner should not parse every slide body until a show is opened (already a trimmed list in spirit; keep it). |
| Remote button to output (LAN) | ≤ 20 ms until the fade starts | Phone remote is a real operator path. |
| Relocatable Linux/macOS/Windows package | `app.asar`-equivalent runtime as small as practical; already far under a bundled Electron asar | Packaging stays a first-class concern. |

Work toward these with measurement, not premature micro-optimization. The compositor is already a CPU painter; profile before moving hot paths to GPU compute. The first wins are still "don't copy frames twice" and "don't decode video on the UI thread" — both of which the architecture already aims at.

---

## 5. Milestones to feature parity

Milestones 1–6 in the original [`PLAN.md`](../PLAN.md) are complete (library settings, remote password, audio routing, stage item types, installers, importer media with listed gaps). The work below starts from that baseline and runs to a Sunday-ready 1.0.

Each milestone should leave `cargo test --workspace` green, update `PARITY.md` and `README.md` in the same commits as behavior, and be small enough to review.

### M1. Quality gates and a honest build — in progress

Restore a compile-clean `main`, document the architecture, and put format/lint/test gates on every OS.

- [x] Fix `StageItemSpec` construction after the Camera fields landed.
- [x] Map FreeShow `type: "camera"` and keep `device.name` / `device.type`.
- [x] Paint camera boxes as a labeled placeholder (live feed is later).
- [x] Make speaker `mix-matrix` updates compatible with GStreamer 1.26.
- [x] Write this architecture / migration plan.
- [x] `rustfmt` the workspace in its own commit so `cargo fmt --check` is meaningful.
- [x] Quality CI: `fmt`, `clippy`, and `test` on Linux, Windows, and macOS, without building installers.
- [x] `rust-toolchain.toml` (stable + rustfmt + clippy) so local and CI toolchains match.

Acceptance: `main` compiles on Linux, Windows, and macOS; `cargo fmt --check`, `cargo clippy --workspace --all-targets --locked`, and `cargo test --workspace --locked` run in CI on all three.

### M2. Operator, settings, and editing depth

The service path exists; the daily editor still feels thinner than Electron.

- [x] Read more of library `settings.json` into `FreeShowPreferences` (language, `timeFormat`, autosave, `labelsDisabled`, `groupNumbers`, `formatNewShow`, `autoOutput`, `maxConnections`, outputs), with tests. Stage clocks, the autosave timer, and slide-group labels honor the matching keys. Still unread or unwired: styles, language UI, display names, `autoOutput`, `maxConnections`, `formatNewShow`, and `labelsDisabled`.
- Theme hover, focus, and border overrides.
- Drawer media as real files (thumbnails, duration, missing-file badges), not names only.
- Richer lyric editing: more than "plain text plus one box moved by 40 px". Item selection, typography, and adding/removing boxes, still writing valid FreeShow JSON.
- Extract `slint!` from `main.rs` when this work lands, if it reduces file size; not as a standalone chore.

Acceptance: a volunteer can open their existing library, recognize the theme and outputs, add a song, edit lyrics in place, and attach a background without dropping fields Electron still reads.

### M3. Compositor, fonts, and stage sources

- System fonts beyond DejaVu / optional Arial: use the item's `font-family` through fontconfig / DirectWrite / Core Text, with a documented fallback chain.
- Remaining typography and effects called out as partial in `PARITY.md` (filters and blends on stills are started; keep going from the highest-impact gaps).
- Live camera, web, and chart boxes on the native stage (today: labeled placeholders). Camera should use the saved `device` name/type against `freeshow-media` discovery.
- Current Output / local stage video already work; do not regress them.

Acceptance: a `stage.json` from Electron renders every box type the Sunday operator actually uses, including a named camera, without falling back to a label unless the device is missing.

### M4. Network media and multi-picture output

- Remaining network transports: OMT, WebRTC.
- NDI groups, multiple independent NDI pictures, and stage NDI (grouped NDI stays disabled today).
- Keep Icecast / RTMP / stereo network routes working; add tests for any new transport at the crate that owns it.
- Stream-only Lessons.church pages that have no direct media URL (today: not-yet).

Acceptance: a church that already routes a stereo mix to NDI/RTMP/Icecast can also send a second picture or a documented new transport without breaking Main.

### M5. Import parity

- ProPresenter 7: remaining graphics, RTF controls, timelines, and unsupported cue actions.
- PowerPoint: remaining color transforms, gradients, linked or tiled media, rich objects.
- Backgrounds from other import formats still listed as incomplete in `PARITY.md`.

Acceptance: the fixtures in `crates/freeshow-data/tests/fixtures` plus a documented set of real `.pro` / `.pptx` samples present with original backgrounds, typography, and slide order. Unsupported features warn; they never crash or silently retarget media.

### M6. Packaging, signing, and the real-app test pass

- Verify the locked three-platform release workflow against one revision and its lockfile.
- Developer ID signing and notarization on macOS; test on more than the current CI host OS versions.
- Measure the performance budgets in §4 on the packaged apps, not only `cargo run`.
- A single "real library" pass: Welcome → open a production-like library → present, stage, remote, record. Close Linear/GitHub issues as they pass. (No ticket process is assumed; keep a short release notes section in `PARITY.md` or a GitHub release.)

Acceptance: a machine without a Rust toolchain can install a signed (or documented unsigned) build and run a Sunday service from an existing FreeShow library.

---

## 6. Working method

- Small conventional commits (`feat:`, `fix:`, `test:`, `docs:`, `chore:`), subjects around 50–72 characters, authored as the project owner. No commit trailers required.
- Each commit leaves the workspace testable. Prefer `cargo fmt` on touched files; after M1 the whole tree should stay rustfmt-clean.
- Update `PARITY.md` and `README.md` in the same commit as behavior, following the existing house style (plain, specific, no marketing).
- When FreeShow behavior is ambiguous, read the Electron source in `../FreeShow-upstream` (or `../FreeShow`) rather than guessing.
- Push to `main` with rebase, never force-push.
- Do not use Cursor Cloud agents for this work; it continues on the local Linux workspace and, when needed, on the owner's machines for signed macOS/Windows verification.

---

## 7. Next (immediately after M1)

1. Finish M1 quality CI and the workspace rustfmt commit.
2. Start M2 with the unread remainder of `settings.json` and drawer media thumbnails — the highest Sunday-operator impact that is still marked partial.
3. Then M3 camera stage sources, reusing the device fields already stored on `StageItemSpec`.
