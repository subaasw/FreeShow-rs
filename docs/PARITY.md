# Parity with FreeShow 1.6.8

Status words for every feature: **done**, **partial**, **not-yet**. A feature is done only when an operator can use it in a Sunday service (or it soft-fails cleanly when an optional runtime is missing). The service path is not done until a user can edit a project, present songs and scripture, run a stage layout, play media, open an existing FreeShow library, and control the show from the remote page.

## Score

- **Feature-weighted parity: 96.3%**
- Sub-features: **86 done / 4 partial / 8 not-yet** (98 leaves).
- Area rows (derived from leaves): **10 done / 7 partial / 0 not-yet** (17 areas).
- Scoring: each leaf has a weight (Sunday-operator impact). `done` scores 1, `partial` scores 1/2, `not-yet` scores 0. The percentage is `100 × Σ(weight × score) / Σ(weight)`. Credential-gated work is included as `not-yet` so the number stays honest about 1.6.8.

## Areas

| Area | Status | Area % | Notes |
| --- | --- | ---: | --- |
| Cargo workspace, GPL-3.0, README | done | 100% | 1 done |
| Slint operator window | done | 100% | 11 done |
| Output and stage windows | partial | 97% | 6 done, 1 partial |
| Theme colors | done | 100% | 3 done |
| Settings file and logging | done | 100% | 1 done |
| Shows, projects, templates, overlays, scripture storage | done | 100% | 6 done |
| Drawer content: shows, media, audio, overlays, templates, scripture, calendar, functions | done | 100% | 9 done |
| Slide editing, project order, go to slide | partial | 93% | 4 done, 1 partial |
| Audience compositor | done | 100% | 7 done |
| Preview matches output | done | 100% | 1 done |
| Images, video, audio, cameras | partial | 92% | 6 done, 1 not-yet |
| Scripture import and search | done | 100% | 1 done |
| Stage layouts | partial | 95% | 3 done, 1 partial |
| Show importers | partial | 94% | 2 done, 1 not-yet |
| Remote and stage web servers | done | 100% | 4 done |
| MIDI, OSC, NDI, timecode, recording, Planning Center, ChurchApps, cloud, OBS, AI | partial | 84% | 18 done, 1 partial, 5 not-yet |
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
| Full Electron settings/editor suite | done | 1.5 | Settings page with General/Display/Theme/Outputs/Styles/Connection/Profiles tabs: Styles adds, picks and edits output styles (background painted on the output), Outputs adds, renames, removes and sets each output's style and window options, and Theme edits colours, font, size and corners, duplicates or resets a theme; all write validated keys to `settings.json` |

### Output and stage windows

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Go/Next fade to output and stage | done | 2 | Configured text transition |
| Screen/monitor picker and memory | done | 1.2 | Header Screen cycles and remembers |
| Second-monitor auto placement | done | 0.8 | Used when no stored choice |
| Installed item font-family | done | 1.2 | fontconfig / DirectWrite / Core Text, DejaVu fallback |
| Output delay frame buffer | done | 1.5 | `special.outputDelayMs`, default 0 so perf stays clean |
| Always-on-top / kiosk on output windows | done | 1.0 | The audience window follows the active output's `alwaysOnTop`/`kiosk` from settings.json at startup, on open and live from Settings → Display |
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
| Access-control `profiles` | done | 0.7 | Password profiles lock the session until `login_profile`; listed in the settings pane |
| Alert-update chrome | done | 0.5 | A background check at startup (when `alertUpdates` is on) puts the newer release in the status bar and a header "Get update" button |

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
| RSS ticker/drawer | done | 0.6 | Tenth drawer tab plus a footer ticker; auto-refresh from `special.rssUrl` / `rssFeeds` |

### Slide editing, project order, go to slide

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Create show/slide and save lyrics plus group | done | 1.5 | Untitled `.show` and empty slides |
| Group numbering and box typography/move | done | 1.5 | 40px nudges in 1920×1080 space and editable stacking order |
| Project New and Go/random/wait actions | done | 1.5 | Wait cancels when leaving the slide |
| Timeline editor cues on the show | done | 1.5 | Zoom plus show-timeline actions |
| Every Electron inspector control | partial | 1 | Inspector controls for font, size, bold, italic, underline, strikethrough, color, align, X/Y/W/H, solid and gradient fills, per-corner radii in pixels or percentages with separate horizontal and vertical values, text and box shadows, line height, letter spacing, picture fit, flips, and crop edges, item and backdrop filters, and slide transitions. The slide editor can insert Media library pictures as editable boxes |

### Audience compositor

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Item styles, backgrounds, media, transitions, clear | done | 3 | Shapes, text, and images retain alpha through transparent output layers. The compositor renders stored rotation with blend modes and backdrop filters, and clips oversized items to the visible output |
| Watermark, output variables, scenes, interactions | done | 2 | `activeScenes` and synced keys |
| Draw-on-output | done | 1 | Stored `drawSettings` |
| Live effects library | done | 1 | `effectsLibrary` washes/particles |
| HTML / web / website boxes | done | 1 | URL preview or stripped HTML snippet |
| Weather boxes | done | 0.8 | Cached Open-Meteo reports auto-refresh (default 600s) and paint into weather boxes |
| Item box-shadow, pixel radius, backdrop-filter | done | 1.2 | Stacked inset/outset `box-shadow` layers, 1–4 values per axis for elliptical `border-radius` in px or %, CSS corner fitting, and `backdrop-filter` (also on rotated items) |

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
| Every Electron stage-item editor pane | partial | 0.5 | Stage editor creates, duplicates, and switches layouts, edits their names and background colors, names and lists items, reorders overlapping boxes, toggles box visibility, selects, moves, and resizes boxes in the preview, edits position, size, font size, color and align, plus each type's own options (slide and notes offsets, line limit, text box, box order and background, clock seconds and date format, which timer to follow, free text), shows a live layout preview with the picked item outlined, and saves `Config/stage.json` (FreeShow `slideOffset`, `clock`, `timer`) without dropping other layouts, unknown fields or styled text runs; media path and fit, tracker appearance, variable selection, QR content and colors, output-picture source and label, camera device and orientation, chart type, data, and pie hole size, and web preview address and still image are editable |

### Show importers

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| ProPresenter, PowerPoint, OpenSong, SongBeamer, and other listed fixtures | done | 3.5 | PowerPoint imports speaker notes for the stage view, custom shows, saved slide ranges, section names, and automatic advance timings, embedded transition sounds with loop and stop cues, and cut, fade, push, wipe, cover, and uncover effects, preserves source cropping for imported pictures and backgrounds and includes linked and tiled backgrounds, mirrored gradient tiles, fractional stops and PowerPoint gradient gamma, linear gradients with angle scaling and circular gradients with point or ellipse focuses and rectangular or shape-path background gradients and direct or theme gradients on positioned shapes, object rotation, picture and gradient flips, nested group transforms, fixed-orientation gradients, and styled runs with mixed-size baselines, Unicode wrapping across formatting changes, text-box insets, optional wrapping, saved normal autofit, paragraph spacing, character bullets, automatic numbering, and first-line and hanging indents within positioned text, and opacity and gamma transforms on background fills. ProPresenter RTF preserves escaped text in declared document and font encodings and omits hidden text from imported lyrics and notes. ProPresenter 7 imports enabled, immediate global clear-audio and clear-background cues. Standalone cues keep the last audience and stage content. Timed next-slide, loop-to-start, and specific-cue jumps run during background playback. Recorded timeline slide cues select the saved arrangement position at the recorded time. See the M6 importer list in `ROADMAP.md` for remaining format details. |
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
| NDI/OMT receive | done | 0.6 | The Media drawer discovers and starts live video backgrounds. Startup configuration remains available. NDI uses the GStreamer plugin. OMT uses the Rust SDK for direct URLs and discovery |
| OMT broadcast | partial | 0.3 | Built-in Rust VMX1 video and routed stereo for audience and stage outputs. Loopback tested. Third-party receiver interoperability remains unverified |
| WebRTC/WHIP send | done | 0.5 | H.264 video and stereo Opus from each saved `network_sub_<output-id>` route |
| Auto-started RTMP | done | 0.5 | From `rtmpData` destinations or `special.autoRtmp`, plus `FREESHOW_RTMP` |
| File recording and Icecast | done | 0.4 | Settings → Icecast edits the connection and can route the Main mix when the library has no saved Icecast route. Network routes can be added during playback. The Rust sender streams stereo Ogg Opus over HTTP or certificate-verified TLS. Settings without a transport choice retain legacy negotiation through optional `shout2send` |
| Virtual camera | done | 0.4 | Linux `v4l2sink`; soft-fail without a loopback device |
| Spout/Syphon | done | 0.3 | Platform-gated; no-ops on Linux |
| Blackmagic Decklink | done | 0.4 | GStreamer decklink sink, no vendor SDK; soft-fail without a card |
| Planning Center plans and ChurchApps plan/lesson import | done | 0.5 |  |
| Folder/URL cloud backup | done | 0.3 |  |
| OBS WebSocket 5 | done | 0.4 | Also reads library `obsData` |
| Functions → AI via Ollama | done | 0.3 | When `FREESHOW_AI` is set |
| GitHub Releases update check | done | 0.3 | Public API, no credentials, off the UI thread; compares to `CARGO_PKG_VERSION`; `FREESHOW_SKIP_UPDATE_CHECK` opts out |
| Download and install a GitHub Release | done | 0.2 | Downloads the platform asset to Downloads, verifies the SHA-256 sidecar (refuses without one), then opens the dmg / shows the installer or archive; no in-place binary swap |
| Spellcheck | done | 0.3 | Lyrics editor shows misspelled words with a suggestion (English, debounced) when a system dictionary is present; embedded list as fallback |
| Stream Deck USB HID | done | 0.2 | Pure-Rust Linux hidraw behind feature `hid` (default off); other OS lists stay empty; no C hidapi |
| Canva / OnStage / Amazing Life providers | not-yet | 0.3 | Credential-gated |
| Google Drive library sync | not-yet | 0.3 | Beyond folder/URL backup; needs an account |
| ChurchApps cloud sync beyond import | not-yet | 0.2 | Needs an account |
| Spotify lyric search | not-yet | 0.2 | Needs an account |
| Cloud or bundled speech-to-text | not-yet | 0.3 | Local model is not bundled; cloud STT needs credentials |

### Installers and crash-safe autosave

| Feature | Status | Weight | Notes |
| --- | --- | ---: | --- |
| Linux/Windows/macOS unsigned or ad-hoc installers in CI | done | 0.8 | `.github/workflows/{release,gate,package-linux,package-windows,package-macos}.yml` (tag-only, see `docs/RELEASING.md`); packaged AppRun/exe/app run `--check-perf`. All three native jobs passed for `e075bea`. [Release run](https://github.com/subaasw/FreeShow-rs/actions/runs/45023447856) |
| Crash-safe autosave | done | 0.5 | Library autosave setting is honored |
| `--check-perf` and `--check-library` budgets | done | 0.4 | Also `--check-service` for a Sunday import/present/REST/Companion/stage path; CI runs `--check-perf` on each packaged runner |
| Apple Developer ID signing and notarization | not-yet | 0.3 | Needs Apple credentials; CI stays unsigned/ad-hoc |

## Remaining

M7 is **not** complete. The lists below are the honest leftovers after the no-credential close-out (update install, weather timer, RSS drawer, NDI/OMT discovery, Stream Deck HID, settings/inspector/stage-editor text panes, `alertUpdates` chrome, profile login, and CI `--check-perf` on packaged Linux/Windows/macOS artifacts). `--check-service` covers a Sunday import/present/REST/Companion/stage path.

### No-credential leftovers

- OMT send and receive need cross-machine tests with third-party implementations.
- Native MediaShout `.mdb` (not a viable pure-Rust path; XML/SSC/JSON dumps import).
- Embedded live PowerPoint playback (import already works).
- A bundled local speech-to-text model (Functions → AI already rewrites a slide through Ollama when `FREESHOW_AI` is set).
- A full visual clone of every Electron inspector control (shadows, line height, letter spacing, filters, transitions) and the media, slide-tracker and variable stage-item panes. Settings now has Theme, Outputs and Styles tabs, and the stage editor has per-type clock, timer, next-slide and text panes.
- Pixel-perfect Electron window chrome (native windows are used). Output-window always-on-top and kiosk are set per output from Settings → Outputs.

### Credential-gated leftovers

- Apple Developer ID signing and notarization (`notarytool` / a Developer ID certificate). CI stays unsigned or ad-hoc.
- Canva presentations and the Canva content provider.
- OnStage and Amazing Life providers.
- Google Drive library sync, beyond backup to a folder or URL.
- ChurchApps cloud sync beyond plan/lesson import.
- Spotify lyric search.
- Cloud speech-to-text.

### Settings stored but not fully used in chrome

- Used in the operator UI or import path: `splitLines`, `customFonts`, `resized` panel widths, header Search, `fullColors`, `slidesOptions` columns/mode, `drawer` height, `shortcuts`, `special.guideSeen` / `special.focusMode` / `special.watermark` / `special.outputDelayMs` (real frame buffer, default 0), `activeScenes`, synced `scenes` / `interactions` / `drawSettings` / `playerVideos`, output variables, `effectsLibrary` live effects, `audioEffects` (gain plus the PCM DSP chain), `timeline` zoom plus show-timeline cues, `alertUpdates` banner, and synced `profiles` login.
- Still stored but not fully used in chrome: `cloudSyncData`, `driveData`, `contentProviderData`, and `ai`. `metronome` and `timecode` drive the footer; `obsData` is used for OBS when `FREESHOW_OBS` is unset.

### Performance check (23 Nov 2026, Asia/Katmandu)

A release `freeshow --check-perf` on the shared Linux box, at 1920×1080, best of five runs: cold open ~12 ms, idle RSS ~31 MB, stills ~377 fps, unsynced 1080p video ~82 fps, Go/Next ~3.0 ms, remote button ~2.9 ms, 1,000-show scan ~11 ms. All under budget. Runs taken while other builds were busy on the box were slower (one run had Go/Next at 55 ms and video at 18 fps), so judge the numbers on a quiet machine. The framed release app with its output and stage windows open idles below 1% CPU on Xvfb.

### Packaging

- Public-distribution Developer ID signing and notarization stay documented and unsigned/ad-hoc in CI, because they need Apple credentials. The locked three-platform installer workflows live in `.github/workflows/{release,gate,package-linux,package-windows,package-macos}.yml` (tag-only, see `docs/RELEASING.md`). Each package job builds an unsigned artifact and runs `--check-perf` on the packaged AppRun, Windows exe, or macOS app. `--check-library` and `--check-service` cover library presentation and a Sunday import/present/REST/Companion/stage path. The compositor paints at 1920×1080 by default, or the active output's stored `bounds` (width is snapped to a multiple of 64 for wgpu row alignment). Slide layout stays 1920×1080.
