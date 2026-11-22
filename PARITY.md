# Parity with FreeShow 1.6.8

Status words for every feature: **done**, **partial**, **not-yet**. A feature is done only when an operator can use it in a Sunday service (or it soft-fails cleanly when an optional runtime is missing). The service path is not done until a user can edit a project, present songs and scripture, run a stage layout, play media, open an existing FreeShow library, and control the show from the remote page.

## Score

- **Feature-weighted parity: 92.4%**
- Sub-features: **77 done / 9 partial / 12 not-yet** (98 leaves).
- Area rows (derived from leaves): **6 done / 11 partial / 0 not-yet** (17 areas).
- Scoring: each leaf has a weight (Sunday-operator impact). `done` scores 1, `partial` scores 1/2, `not-yet` scores 0. The percentage is `100 × Σ(weight × score) / Σ(weight)`. Credential-gated work is included as `not-yet` so the number stays honest about 1.6.8.

## Areas

| Area | Status | Area % | Notes |
| --- | --- | ---: | --- |
| Cargo workspace, GPL-3.0, README | done | 100% | 1 done |
| Slint operator window | partial | 93% | 10 done, 1 partial |
| Output and stage windows | partial | 92% | 5 done, 2 partial |
| Theme colors | done | 100% | 3 done |
| Settings file and logging | done | 100% | 1 done |
| Shows, projects, templates, overlays, scripture storage | partial | 89% | 4 done, 1 partial, 1 not-yet |
| Drawer content: shows, media, audio, overlays, templates, scripture, calendar, functions | partial | 92% | 8 done, 1 not-yet |
| Slide editing, project order, go to slide | partial | 93% | 4 done, 1 partial |
| Audience compositor | partial | 90% | 5 done, 2 partial |
| Preview matches output | done | 100% | 1 done |
| Images, video, audio, cameras | partial | 92% | 6 done, 1 not-yet |
| Scripture import and search | done | 100% | 1 done |
| Stage layouts | partial | 95% | 3 done, 1 partial |
| Show importers | partial | 94% | 2 done, 1 not-yet |
| Remote and stage web servers | done | 100% | 4 done |
| MIDI, OSC, NDI, timecode, recording, Planning Center, ChurchApps, cloud, OBS, AI | partial | 79% | 16 done, 1 partial, 7 not-yet |
| Installers and crash-safe autosave | partial | 85% | 3 done, 1 not-yet |

## Sub-features

### Cargo workspace, GPL-3.0, README

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Workspace, license, and README | done | 2 | `freeshow-rust` beside the Electron repo |

### Slint operator window

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Drawer, show list, and slide list | done | 1.5 | All library drawer tabs list entries |
| Lyric editor and text-box typography | done | 1.5 | Edits save back to the `.show` file |
| Project order and Go/Next chrome | done | 1.5 | New project, reorder, publish the slide |
| Operator preview | done | 1 | Preview follows the audience picture |
| Undo/redo of lyric edits | done | 0.8 | History is 40 steps |
| Shortcut editor and bindings | done | 0.8 | Stored in `settings.json` and honored |
| Focus/presenter mode, first-run guide | done | 0.6 | `special.focusMode` / `guideSeen` |
| Drawer height, always-on-top, kiosk | done | 0.6 | Bottom drawer height and window flags |
| Named window-layout profiles | done | 0.7 | Stored placements for operator/output/stage |
| Header Search and panel widths | done | 0.5 | `resized` left/right widths |
| Full Electron settings/editor suite | partial | 1.5 | Not every 1.6.8 settings pane or inspector field is cloned |

### Output and stage windows

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Go/Next fade to output and stage | done | 2 | Configured text transition |
| Screen/monitor picker and memory | done | 1.2 | Header Screen cycles and remembers |
| Second-monitor auto placement | done | 0.8 | Used when no stored choice |
| Installed item font-family | done | 1.2 | fontconfig / DirectWrite / Core Text, DejaVu fallback |
| Output delay frame buffer | done | 1.5 | `special.outputDelayMs`, default 0 so perf stays clean |
| Always-on-top / kiosk on output windows | partial | 0.8 | Flags exist on the operator; not every output-window Electron option |
| Pixel-perfect Electron window chrome | partial | 0.5 | Native windows, not the Electron shell |

### Theme colors

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Eleven built-in themes | done | 1.2 | Match `defaultThemes.ts` |
| Library and custom themes.json | done | 1.2 | Colors, font, size, hover, focus, radius |
| Operator washes and radius | done | 0.6 | Buttons, tabs, list rows |

### Settings file and logging

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| App config, logging, and library keys | done | 4 | Used keys are listed in this file's settings section |

### Shows, projects, templates, overlays, scripture storage

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Load shows, projects, templates, overlays, events, synced settings | done | 2.5 | Existing FreeShow libraries open |
| Load `.fsb` Bibles | done | 1 | Scripture tab and search |
| Save lyric edits and project order | done | 2 | Writes the same files Electron uses |
| Export text of the open show | done | 0.8 | `Exports/` in the text-importer shape |
| Access-control `profiles` | partial | 0.7 | Synced accounts are stored, not a full login UI |
| Alert-update chrome | not-yet | 0.5 | `alertUpdates` is stored but not shown |

### Drawer content: shows, media, audio, overlays, templates, scripture, calendar, functions

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Shows, media, audio, overlays, templates, scripture, calendar, functions tabs | done | 2 | Each tab lists library entries |
| Calendar auto-start of a due show | done | 0.6 | Including older type `show` events |
| Apply template onto the open song | done | 0.6 | Keeps lyrics |
| Scripture search on `.fsb` | done | 0.6 |  |
| Media metadata and thumbnails | done | 1 | Size, duration, missing badge, still thumbs |
| Player tab | done | 0.8 | Stored `playerVideos` |
| Functions: Clear/Next/Previous/Timer/Record/Export/Screenshot | done | 1 | PDF, package, and PNG exports included |
| Functions: Planning Center, ChurchApps, Backup, Restore, OBS, AI, Icecast | done | 0.8 | No-credential paths only |
| RSS ticker/drawer | not-yet | 0.6 | Parse/fetch helpers exist; no operator RSS list yet |

### Slide editing, project order, go to slide

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Create show/slide and save lyrics plus group | done | 1.5 | Untitled `.show` and empty slides |
| Group numbering and box typography/move | done | 1.5 | 40px nudges in 1920×1080 space |
| Project New and Go/random/wait actions | done | 1.5 | Wait cancels when leaving the slide |
| Timeline editor cues on the show | done | 1.5 | Zoom plus show-timeline actions |
| Every Electron inspector control | partial | 1 | Common typography and box moves; not every 1.6.8 inspector field |

### Audience compositor

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Item styles, backgrounds, media, transitions, clear | done | 3 | Core Sunday path |
| Watermark, output variables, scenes, interactions | done | 2 | `activeScenes` and synced keys |
| Draw-on-output | done | 1 | Stored `drawSettings` |
| Live effects library | done | 1 | `effectsLibrary` washes/particles |
| HTML / web / website boxes | done | 1 | URL preview or stripped HTML snippet |
| Weather boxes | partial | 0.8 | Card from stored values; Open-Meteo fetch is implemented but not auto-refreshed on a timer |
| Item box-shadow, pixel radius, backdrop-filter | partial | 1.2 | Called out from M3; some typography extras still differ from Electron |

### Preview matches output

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Operator preview uses the audience frame | done | 3 | Same compositor |

### Images, video, audio, cameras

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Still images, including remote HTTP | done | 1.5 |  |
| Local and direct-URL video | done | 2 | GStreamer uridecodebin |
| YouTube/Vimeo via yt-dlp | done | 1.5 | Resolves `yt-dlp -g` when the binary is present, then plays the URL |
| Audio playback and routing | done | 1.5 | cpal + network routes |
| Audio-effects DSP chain | done | 1.5 | Gain, peaking EQ, delay, compressor, limiter on decoded PCM |
| Cameras as underlay/stage sources | done | 1 | GStreamer device list |
| Embedded live PowerPoint playback | not-yet | 0.8 | Import exists; a running PPT process is not hosted |

### Scripture import and search

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Import and search `.fsb` plus other scripture importers | done | 4 | See the importers row for file types |

### Stage layouts

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Load and paint stage layouts | done | 2 | Includes next-slide, notes, clock, timer |
| Stage cameras, web, and chart boxes | done | 1.5 |  |
| Stage as a second NDI/WebRTC picture | done | 1 |  |
| Every Electron stage-item editor pane | partial | 0.5 | Layouts paint; not every stage editor field is cloned |

### Show importers

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| ProPresenter, PowerPoint, OpenSong, SongBeamer, and other listed fixtures | done | 3.5 | See M5 / M7 importer list |
| MediaShout XML/SSC/JSON, Quelea, SoftProjector, EasySlides, VerseView, VideoPsalm, CSV, image-only, project packages | done | 1.2 | Fixture tests for each |
| Native MediaShout `.mdb` | not-yet | 0.3 | Not a viable pure-Rust path |

### Remote and stage web servers

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Remote page, WebSocket, and Socket.IO | done | 2 | Password via header or Bearer |
| Stage page with native frames | done | 1.5 |  |
| Webhook/REST API | done | 1 | `/api/*`, `/webhook`; short names map to `API:` channels |
| Stream Deck / Bitfocus Companion over HTTP+WS | done | 0.5 | Same remote port, `/companion` |

### MIDI, OSC, NDI, timecode, recording, Planning Center, ChurchApps, cloud, OBS, AI

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| MIDI and OSC input | done | 0.6 |  |
| Timecode (MTC/LTC) | done | 0.4 |  |
| NDI send, including stage and groups | done | 0.8 | One sender per (group, stage); source name `{name} [{group}]` |
| NDI/OMT receive | done | 0.6 | Soft-fails when the GStreamer plugin is missing |
| OMT broadcast | partial | 0.3 | Configured, but the runtime is not bundled |
| WebRTC/WHIP send | done | 0.5 |  |
| Auto-started RTMP | done | 0.5 | From `rtmpData` destinations or `special.autoRtmp`, plus `FREESHOW_RTMP` |
| File recording and Icecast | done | 0.4 |  |
| Virtual camera | done | 0.4 | Linux `v4l2sink`; soft-fail without a loopback device |
| Spout/Syphon | done | 0.3 | Platform-gated; no-ops on Linux |
| Blackmagic Decklink | done | 0.4 | GStreamer decklink sink, no vendor SDK; soft-fail without a card |
| Planning Center plans and ChurchApps plan/lesson import | done | 0.5 |  |
| Folder/URL cloud backup | done | 0.3 |  |
| OBS WebSocket 5 | done | 0.4 | Also reads library `obsData` |
| Functions → AI via Ollama | done | 0.3 | When `FREESHOW_AI` is set |
| GitHub Releases update check | done | 0.3 | Public API, no credentials; compares to `CARGO_PKG_VERSION` |
| Download and install a GitHub Release | not-yet | 0.2 | Check only; the running binary is not replaced |
| Spellcheck | done | 0.3 | Embedded word list plus an optional system dictionary |
| Stream Deck USB HID | not-yet | 0.2 | Companion/HTTP+WS is the supported path |
| Canva / OnStage / Amazing Life providers | not-yet | 0.3 | Credential-gated |
| Google Drive library sync | not-yet | 0.3 | Beyond folder/URL backup; needs an account |
| ChurchApps cloud sync beyond import | not-yet | 0.2 | Needs an account |
| Spotify lyric search | not-yet | 0.2 | Needs an account |
| Cloud or bundled speech-to-text | not-yet | 0.3 | Local model is not bundled; cloud STT needs credentials |

### Installers and crash-safe autosave

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Linux/Windows/macOS unsigned or ad-hoc installers in CI | done | 0.8 | `.github/workflows/{linux,windows,release}.yml` |
| Crash-safe autosave | done | 0.5 | Library autosave setting is honored |
| `--check-perf` and `--check-library` budgets | done | 0.4 | Including remote-button latency ≤16 ms |
| Apple Developer ID signing and notarization | not-yet | 0.3 | Needs Apple credentials; CI stays unsigned/ad-hoc |

## Remaining

M7 is **not** complete. The lists below are the honest leftovers after groups 4 and 5 and the output-delay / audio-DSP / YouTube-Vimeo pass.

### No-credential leftovers

- Apply a GitHub Release (download and replace the running binary). The public Releases check is in.
- Auto-refresh weather boxes from Open-Meteo on a timer (parse, fetch, and stored-value cards are in).
- An operator RSS drawer or ticker (parse/fetch helpers are in).
- OMT broadcast, which still needs the unbundled Open Media Transport runtime.
- NDI/OMT source discovery lists (receive-by-name works when the GStreamer plugin is present; this box has no `ndisrc`).
- Stream Deck USB HID (Companion/HTTP+WS on the remote port is the supported path).
- Native MediaShout `.mdb` (not a viable pure-Rust path; XML/SSC/JSON dumps import).
- Embedded live PowerPoint playback (import already works).
- A bundled local speech-to-text model (Functions → AI already rewrites a slide through Ollama when `FREESHOW_AI` is set).
- A full clone of every Electron settings/inspector/stage-editor pane, `alertUpdates` chrome, and access-control profile login UI.
- Measuring the same `--check-perf` budgets on packaged Windows and macOS apps (Linux release budgets already run here).

### Credential-gated leftovers

- Apple Developer ID signing and notarization (`notarytool` / a Developer ID certificate). CI stays unsigned or ad-hoc.
- Canva presentations and the Canva content provider.
- OnStage and Amazing Life providers.
- Google Drive library sync, beyond backup to a folder or URL.
- ChurchApps cloud sync beyond plan/lesson import.
- Spotify lyric search.
- Cloud speech-to-text.

### Settings stored but not fully used in chrome

- Used in the operator UI or import path: `splitLines`, `customFonts`, `resized` panel widths, header Search, `fullColors`, `slidesOptions` columns/mode, `drawer` height, `shortcuts`, `special.guideSeen` / `special.focusMode` / `special.watermark` / `special.outputDelayMs` (real frame buffer, default 0), `activeScenes`, synced `scenes` / `interactions` / `drawSettings` / `playerVideos`, output variables, `effectsLibrary` live effects, `audioEffects` (gain plus the PCM DSP chain), and `timeline` zoom plus show-timeline cues.
- Still stored but not fully used in chrome: `alertUpdates`, `cloudSyncData`, `driveData`, `contentProviderData`, and `ai`. `metronome` and `timecode` drive the footer; `obsData` is used for OBS when `FREESHOW_OBS` is unset. Synced `profiles` remain stored access-control accounts.

### Packaging

- Public-distribution Developer ID signing and notarization stay documented and unsigned/ad-hoc in CI, because they need Apple credentials. The locked three-platform installer workflows already exist in `.github/workflows/{linux,windows,release}.yml`. `--check-perf` and `--check-library` measure the plan budgets, including remote-button latency (handle `API:next_slide`, apply it, and paint the audience frame) against a ≤16 ms budget. The compositor paints at 1920×1080 by default, or the active output's stored `bounds` (width is snapped to a multiple of 64 for wgpu row alignment). Slide layout stays 1920×1080.
