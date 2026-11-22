# freeshow-rust

Native Rust rewrite of [FreeShow](https://freeshow.app/). The Electron app stays the behavior reference until this project can run a service on its own.

GPL-3.0-only.

## Run

Install Rust 1.92 or newer and GStreamer 1.x (macOS: `brew install gstreamer libnice-gstreamer`). On Apple Silicon, point pkg-config at Homebrew before building: `export PKG_CONFIG_PATH="$(brew --prefix)/lib/pkgconfig"`. Video and cameras use GStreamer. Camera selection opens the device returned by native discovery rather than guessing its device index. Compressed camera feeds use a software decoder to produce CPU-readable frames. A disconnected camera reports an error; raw and JPEG camera feeds share the video decoder. Physical-camera testing remains pending. Audio uses the system output device. Then from this directory:

```sh
cargo run -p freeshow
```

Point it at an existing FreeShow library, or open one show:

```sh
cargo run -p freeshow -- --data ~/Documents/FreeShow
cargo run -p freeshow -- --show ~/Documents/FreeShow/Shows/Some\ Song.show
```

`scripts/present-sample.sh` builds a one-song library and opens the first slide on the output window.

Audience slides and the native stage window render saved FreeShow QR boxes with their configured payload and colors. The code stays square in a rectangular box. A Current Output box previews the active audience picture on the native stage and its browser view, with the saved output name when requested. Stage text boxes honor saved text-box numbers, item order, and stage bindings for multilingual slides.

Put song sources in the library `Imports/` folder before opening the library. ProPresenter 4–6 XML (`.pro4`, `.pro5`, `.pro6`) and `.proBundle` archives import lyrics, group children, disabled slides, notes, and slide backgrounds. Bundled media is copied into `Media/` and the saved shows point there; existing files are kept, and matching media is reused on repeat imports. Missing or ambiguous bundle media produces an import warning instead of linking to another file. ProPresenter 7 `.pro` files and bundles retain cue groups, the selected arrangement, slide notes, backgrounds, and separate text boxes with saved bounds and box-level typography. Mixed RTF runs preserve fonts, sizes, colors, bold, italic, underline, strikethrough, paragraph alignment, all caps, and letter spacing within the box. Legacy RTF hex escapes use the declared document code page or font charset. Imported lyrics and notes omit hidden RTF text. Solid rectangles and ellipses retain their size, position, and layer order. The Rust compositor applies stored rotation to ordinary slide items. Timed next-slide cues advance within the selected arrangement. Other graphics, timelines, and unsupported cue actions remain incomplete. Bundles are limited to 10,000 entries, 16 MiB per presentation, 512 MiB per media file, and 2 GiB uncompressed in total. MediaShout XML/SSC and JSON table dumps, Quelea XML/`qsp`, SoftProjector `.sps`, EasySlides, VerseView, VideoPsalm, CSV, image-only shows, and FreeShow project packages (JSON or zip) also import from `Imports/`. Native MediaShout `.mdb` and Canva presentations are not imported.

PowerPoint `.pptx` imports keep the presentation's saved slide order and hidden slides. PNG and JPEG backgrounds can be inherited from a layout or master. Embedded images are copied into `Media/` with their stretch and crop. Linked image backgrounds keep their URL, and tiled backgrounds repeat across the slide. Theme fills use the deck’s style matrix and the active slide color map. Backgrounds preserve tint, shade, luminance, opacity, and gamma adjustments, along with linear and circular gradients. The Rust compositor preserves radial point focuses inside the outer circle and rectangular focuses inside the slide for direct and theme fills. Extended focus geometry and richer slide objects remain incomplete.

While the app is running, a phone on the same network can open `http://<this-computer>:5510/` to change slides. A stage page is at `http://<this-computer>:5511/`. It shows the native stage layout, including its clocks, timers, notes, still images, and QR codes. The ports come from the library `settings.json` when it sets `ports.remote` and `ports.stage`. Native stage layouts honor next-slide boxes and clock dates saved in older FreeShow libraries. Local stage video boxes loop silently. A slide-text box with `includeMedia` mirrors the current audience background or shows the still background of its offset slide. The stage port serves layouts and current output and ignores show-control commands. Both speak FreeShow's message shape, `{ "channel", "data" }`, on a WebSocket at `/ws` and on the Socket.IO events `REMOTE` (remote port) and `STAGE` (stage port). When the library `settings.json` has `serverData.remote.password`, both pages ask for it and every socket connection must send `ACCESS` with that password before any channel is answered. The password is sent as the `x-freeshow-password` header on the page's HTTP requests. Channels that move the show are `API:next_slide`, `API:previous_slide`, `API:index_select_slide`, `API:clear_all`, `SHOW`, and `API:start_scripture`. `GET_SCRIPTURE` and `SEARCH_SCRIPTURE` read the Bibles in the library. `API:scripture_next` moves to the next verse. On connect the remote also receives `PROJECTS`, `FOLDERS`, and `PROJECT`. `SEARCH_SHOWS` finds a song, and `API:set_plain_text` saves lyric slides separated by a blank line. `API:create_project`, `API:rename_project`, `API:delete_project`, and `API:remove_project_item` edit the service list. `API:get_groups` lists verse and chorus groups. `API:change_layout` switches a song to another layout. `GET_TEMPLATES` and `API:set_template` list templates and apply one to the open song. `API:id_start_timer`, `API:id_pause_timer`, and `API:id_stop_timer` run a countdown from the library. `API:id_select_overlay` draws an overlay on the audience. `API:play_media` plays a file from the library, `API:pause_audio` holds it, `API:audio_seekto` and `API:video_seekto` move the playhead, `API:toggle_media_loop` turns background looping on or off, and the volume slider on the phone page sets the main output level.

Audio files and video sound follow the library's `audioRouting.connections` graph through `audioChannelsData` mixer gains to the default speaker or named speaker devices. Slide audio and drawer audio use `drawer_audio`; playlists use both `playlist_sub_<id>` and `playlists_default`. A source with no speaker connection stays silent. The phone page lists mixer channels and lets you set each channel's volume and mute independently. Main affects paths through the Main channel. Video sound uses `output_window` and `output_win_sub_<audience-output-id>` and keeps the clip's stored volume.

FreeShow's named speaker IDs come from Chromium and need a native device mapping. Run `cargo run -p freeshow -- --list-audio-devices` for audio-file devices or `cargo run -p freeshow -- --list-video-audio-devices` for video speaker devices, then add the exact device name to this app's `settings.json`, for example:

```json
{"audio_devices": {"speaker_sub_<FreeShow-device-id>": "Stage Speakers"}}
```

Keep the other app settings when adding that field. An unavailable mapped device logs an error and does not fall back to the room speakers. Audio-file speaker devices and network feeds use independent playback clocks. Video sound uses GStreamer speaker sinks on its playback clock. Hardware speaker connections honor `channelIndex`/`toChannelIndex` and an explicit `fromChannelIndex`. A single selected speaker port receives a mono mix; multiple selected ports preserve the corresponding source channels. Named speaker connections without an index use port 0, as FreeShow does. Audio and video share the same channel matrix, and unmapped ports stay silent. Changes to video channel routes and their mixer gains take effect during playback. Active video audio branches apply the latest queued route on the streaming thread when the next buffer arrives. Indices are zero-based and must be below 64; unavailable device channels fail without falling back to the room speakers. Stereo network feeds follow `network_sub_<output-id>` routes and keep audio-file and video sources separate until the sender mixes them. A network-only audio file needs no physical output device. The PCM effects chain supports gain, peaking EQ, delay, compressor, and limiter. OMT transport requires its separate runtime.

Icecast sends stereo Ogg Opus through the saved audio route to the `icecast` node. It reads `special.icecast` from the library's `settings.json` and only opens a configured stream when the graph has an Icecast connection and the node is enabled. **Functions → Enable/Disable Icecast**, slide actions, and the password-gated `API:toggle_icecast` command update that saved setting. It requires the GStreamer `shout2send` plugin. Windows builds can stream when their SDK includes it.

OSC input listens on UDP port 5505, the same port FreeShow uses. `/freeshow/slide/next`, `/freeshow/slide/previous`, `/freeshow/slide/1`, `/freeshow/clear/all`, and `/freeshow/show/<id>/start` drive the open show.

MIDI input opens the first connected input, or the port named by `FREESHOW_MIDI_IN`. FreeShow's default notes are used: channel 1 note 0 is next, note 1 is previous, channel 3 note 0 clears, and channel 5 note 2 selects a slide from the velocity.

Each enabled, ungrouped NDI output in the library sends its picture and routed stereo audio through the GStreamer NDI plugin. Audience outputs send the audience picture; stage outputs send the stage picture. A source name uses `ndiData.name`, or `FreeShow NDI - <output name>`. With no enabled NDI output, nothing is broadcast. The plugin and NDI SDK runtime are required. Grouped outputs stay disabled because the current sink does not expose NDI discovery groups. Enabled OMT and WebRTC (WHIP) outputs are read from the library, including stage pictures. OMT needs the Open Media Transport runtime, which is not bundled, so nothing is sent yet. Enabled WebRTC outputs encode H.264 and stereo Opus through webrtcbin. Each WHIP stream uses the saved mix routed to `network_sub_<output-id>` and publishes to `webrtcData.url` with an optional Bearer token.

MIDI timecode uses the first input, or the port named by `FREESHOW_MTC_IN`. Quarter-frames and full frames replace the stage clock with `HH:MM:SS:FF` while they are arriving. Linear timecode listens only when `FREESHOW_LTC_IN` names an input (`default` uses the system input) and uses 25 fps unless `FREESHOW_LTC_FPS` is set.

Functions → Record writes the audience picture to `Recordings/freeshow-YYYYMMDD-HHMMSS.mp4` in the library. Choosing it again, or closing the app, closes the file. Local MP4 recording is still video only. `FREESHOW_RTMP` streams the current picture with routed stereo AAC audio to that URL. It uses the first enabled RTMP output ID in the library, or `default` when none is configured; route to `network_sub_<id>` to feed it. Saved `rtmpData.destinations` can start through their auto-start flags or `special.autoRtmp`. The app supports one active RTMP destination. Static pictures repeat at 30 fps so media audio can continue without slide changes. The RTMP pipeline corrects source timing gaps up to 250 ms. Network feeds require the GStreamer `inter` plugin; RTMP also requires an AAC encoder (`avenc_aac`).

Planning Center loads when `FREESHOW_PCO_TOKEN` is a Services access token and `FREESHOW_PCO_PLAN` is `serviceTypeId/planId`. `FREESHOW_PCO_FILE` can point at a saved plan document instead. Songs and plan items become shows, headers stay section labels in the project, and Functions → Planning Center loads the plan again. With neither a token nor a file, the app does not contact Planning Center.

ChurchApps loads the presenter plans for `FREESHOW_CHURCHAPPS_TOKEN`, or a saved plan from `FREESHOW_CHURCHAPPS_FILE`. Arrangement lyrics become shows, with later lyric blocks kept as child slides, and Functions → ChurchApps loads them again. Without a token, service-plan API requests are disabled.

Lessons.church `.olp`, `.olf`, and lesson `.json` files in `Imports/` are imported at startup or through Functions → ChurchApps, without an account. `FREESHOW_CHURCHAPPS_FILE` can also point at one lesson document. Cover images and media from playlist groups or OLF `play` actions download over HTTP/HTTPS into `Media/Lessons/`. The resulting shows are added to the `Lessons.church` project only after every media download finishes. Grouped slides keep video loop flags, audible video sound, and advancement after a non-looping video except on the final slide. Completed downloads are cached for offline reuse; repeated imports keep operator edits. Failed or truncated transfers do not publish a lesson and can be retried. Limits are 16 MiB per source file, 2,000 media slides, and 2 GiB per media file. Downloads run in the background at startup or after the Functions action. The footer shows downloaded bytes for the current file and offers Cancel import. Cancellation waits for any in-flight network read to return before cleaning up the partial file. Completed media stays cached. Service-plan publication keeps unrelated project edits and skips the refresh if an imported project changed during preparation. Service-plan lesson sections, actions, and add-ons also use this local media cache. Venue plans resolve their public feed, and a venue service with no custom order follows its playable feed actions. Uncached add-ons use the public add-on lookup. Service-plan media stays in service order, keeps its explicit loop flags, and stores scheduled durations. Failed metadata or media requests keep the previous service and show files. Saved service-plan documents must include the resolved `files` arrays on lesson children. Public lesson and media requests do not carry the service account token. Vimeo, YouTube, and other stream-only items are published with their remote URL as the media path and are not downloaded. Direct HTTP video files play through GStreamer. YouTube and Vimeo pages can play when `yt-dlp` is installed and resolves a playable stream.

Functions → Backup writes `Backups/YYYY-MM-DD_HH-MM.zip` with shows, portable config, and bibles in the FreeShow backup names. Functions → Restore puts the newest of those zips back into the library. `FREESHOW_CLOUD_DIR` also receives a copy, and `FREESHOW_CLOUD_URL` receives a PUT of the zip (`FREESHOW_CLOUD_TOKEN` is sent as a bearer token when it is set). A failed copy or upload keeps the local zip and the show continues. When neither destination is set, the app does not upload on startup. Functions → Export PDF, Export package, and Screenshot write a lyrics PDF, a `.project` zip of the open project, and a PNG of the current audience frame under `Exports/`. Functions → Metronome starts and stops a beat from the stored `metronome.tempo` (or `bpm`) and `beats`; the footer also shows a configured `timecode` mode. When `fullColors` is on, slide labels use the group color as a fill; otherwise a color bar marks the group. `slidesOptions.columns` and `mode` lay the slide list out as a grid, except `list` and `lyrics`, which stay one column. The show/media drawer sits along the bottom at `drawer.height` (default 300px) and is hidden in focus mode. A first-run guide appears until dismissed. Functions → Undo / Redo, Focus mode, Guide, Kiosk, Always on top, and Shortcuts match the default chords Ctrl+Z, Ctrl+Y, Ctrl+Shift+F, F1, F11, and Ctrl+Shift+A; Functions → Shortcuts edits those bindings.

OBS uses WebSocket 5. `FREESHOW_OBS` is `host:port` (port 4455 when the host has none) and `FREESHOW_OBS_PASSWORD` is the server password. When those are unset, the library `obsData` (`ip` or `host`, `port`, `password`) is used if it is enabled. Functions → OBS scene sends `SetCurrentProgramScene` for `FREESHOW_OBS_SCENE`. Functions → OBS stream and OBS record start and then stop that output. When `FREESHOW_OBS` and `FREESHOW_OBS_SCENE` are both set, the scene is selected at startup. A closed OBS is logged and the show continues.

Functions → AI rewrites the selected slide through Ollama when `FREESHOW_AI` is `ollama` or a base URL such as `http://127.0.0.1:11434`. `FREESHOW_AI_MODEL` defaults to `gemma3:4b`, and `FREESHOW_AI_PROMPT` is the instruction. The model replies with slide lines, which replace the selected slide. With `FREESHOW_AI` unset, the app does not contact a model.

Every 15 minutes the open shows and the project file are saved again. A file whose bytes did not change is left alone, so the `Backups` folder does not grow. Each save still writes a temporary file, flushes it, and renames it into place.

## Release build

`scripts/package.sh` builds the locked release binary and stages `dist/freeshow-<target>/` with the binary, `LICENSE`, `README.md`, and `BUILD.txt` (compiler version, `SOURCE_DATE_EPOCH`, and the Cargo.lock hash). Paths inside the binary are remapped to `/freeshow`.

```sh
scripts/package.sh
scripts/package.sh x86_64-pc-windows-msvc
scripts/package.sh x86_64-unknown-linux-gnu
```

The extra arguments need that Rust target installed, plus a GStreamer development package for the platform. For the native Windows installer, use the PowerShell build below. Linux packaging produces a runtime archive. Ubuntu 24.04 x86-64 CI verified relocation, desktop installation and removal, and H.264 playback after removing system GStreamer.

On a native Mac, the script also creates `FreeShow Rust.app` and a compressed `.dmg` with an Applications shortcut and SHA-256 checksum. Building needs Python 3.11+, Xcode command-line tools, and Homebrew GStreamer. The app bundles its linked libraries, selected GStreamer plugins, and plugin scanner, so the installed app needs neither Rust nor Homebrew. The optional NDI SDK remains a separate installation. Each app records its architecture and measured minimum macOS version in `Contents/Resources/BUILD.json`. Build on a host with dependencies compatible with the macOS versions you intend to support.

The bundle includes this project's source archive, GPL license, DejaVu license, and available third-party license files/Homebrew source recipes. Source files must be tracked and the working tree must match the Git index; use a clean checkout for release builds. The imported FreeShow icon is credited in `ICON-SOURCE.txt`.

Headless checks (no window required) include `--check-runtime`, `--check-media <file>`, `--check-perf [library]`, and `--check-library [library]`. `--check-perf` measures cold open, resident memory on Linux, macOS, and Windows, compositor frame rates, Go/Next, remote-button latency, and a 1,000-show scan. It can generate its video sample from the packaged GStreamer runtime. `scripts/bench-perf.py` wraps the same check. `--check-library` builds or opens a Sunday-style library, presents a song, fingerprints the output frames, and GETs the remote and stage pages.

Packaging relocates the app into a path with spaces, cold-scans its plugins with an empty home/cache, and decodes an H.264 sample from disk and over loopback HTTP without opening windows. Where `sandbox-exec` is available, these checks deny reads from Homebrew and Rust toolchain directories. The DMG is verified, mounted read-only, and checked again. Results go into `macos-verification.json`. Native macOS 26 CI verified the Apple Silicon app and mounted disk image. The default signature is ad-hoc for development. Public-distribution Developer ID signing and notarization are not performed in CI, because they need Apple credentials that only the project owner can provide. Unsigned (or ad-hoc signed) Linux, Windows, and macOS installers are the verified path. To sign and notarize a macOS build with a Developer ID Application certificate and an App Store Connect API key:

```sh
python3 scripts/package-macos.py --binary target/release/freeshow --output dist/macos --identity "Developer ID Application: Your Name (TEAMID)" --dmg
xcrun notarytool submit dist/macos/*.dmg --key AuthKey_XXXXXX.p8 --key-id KEYID --issuer ISSUER_UUID --wait
xcrun stapler staple dist/macos/*.dmg
```

`notarytool` needs network access to Apple. Those credentials stay with the owner; they are not stored in this repository or in CI secrets.

On a native Linux build host, `scripts/package.sh` also prepares a `.tar.gz` containing `AppRun`, the binary, relocated shared libraries, selected GStreamer plugins, the plugin scanner, desktop metadata, an icon, source/provenance, and third-party notices. It needs Python 3.11+, patchelf, binutils, desktop-file-utils, GStreamer development/runtime packages, and the usual X11/Wayland development libraries. The Linux workflow in `.github/workflows/linux.yml` lists the Ubuntu dependencies and is the native verification gate. Base glibc, graphics drivers, and desktop/audio services remain OS requirements; the measured minimum glibc version is recorded in `share/BUILD.json`. The optional NDI SDK remains separate.

Extract the Linux archive into a permanent folder, run `./AppRun` to launch directly, or `./install.sh` to add a per-user application-menu entry and `~/.local/bin/freeshow-rust`. `./install.sh --uninstall` removes that desktop integration and keeps your library. Moving the extracted folder later requires running its installer again. The desktop entry uses `/usr/bin/env` to launch the quoted application path, including folders containing a literal percent sign that GLib otherwise rejects during executable validation.

The Linux verifier checks relocation into a path with spaces, quotes, percent signs and Unicode; plugin discovery and H.264 decoding with an empty home/cache; the actual desktop-entry parser; and desktop removal without deleting a library sentinel. Native CI also removes system GStreamer packages and repeats decoding, then uploads the archive, checksum and reports as a workflow artifact. The **Three-platform release packages** workflow in `.github/workflows/release.yml` builds Linux x86-64, Windows x86-64, and macOS ARM64 packages from one Git revision and the checked-in Cargo lockfile. It runs on version tags, changes to packaging or media code, and manual runs. Each platform uses its native packager and uploads unsigned or ad-hoc signed artifacts to the run. All three packages passed native verification at revision `e075bea`. Use the steps above on a Mac with the owner's Apple credentials for Developer ID notarization. [Verified release run](https://github.com/subaasw/FreeShow-rs/actions/runs/45023447856)

On a native x64 Windows host, build the locked release with the MSVC GStreamer SDK, then run the installer packager. It needs Python 3.11+, Visual Studio with the x64 C++ tools and redistributable files, LLVM, and Inno Setup 6. The Windows workflow installs the pinned SDK and shows the build environment variables.

```powershell
cargo build --locked --release -p freeshow
python scripts/package-windows.py --gst C:\gstreamer\1.0\msvc_x86_64
```

The unsigned setup executable installs to the current user's Programs folder and adds a Start menu shortcut. It carries the selected GStreamer DLLs, plugins, and scanner, the app-local Visual C++ runtime, source, notices, and provenance. Native Windows CI verified installation, H.264 decoding with a cold registry and the build SDK hidden, and uninstall.

Headless diagnostics also work on a development binary:

```sh
cargo run -p freeshow -- --check-runtime
cargo run -p freeshow -- --check-media /path/to/video.mp4
cargo run --release -p freeshow -- --check-perf
cargo run --release -p freeshow -- --check-library
```

Check the workspace:

```sh
cargo test --workspace
cargo build --workspace
```

Optional transport checks:

```sh
cargo test -p freeshow-ndi the_combiner_attaches_audio -- --ignored
# In another terminal, start a loopback RTMP listener with ffmpeg:
ffmpeg -nostdin -y -listen 1 -i rtmp://127.0.0.1:19350/live/freeshow -c copy /tmp/freeshow-rtmp-smoke.flv
FREESHOW_TEST_RTMP=rtmp://127.0.0.1:19350/live/freeshow cargo test -p freeshow-record publish_to_local_rtmp_listener -- --ignored
```

The combiner check needs the NDI plugin but not the SDK; NDI receiver playback still needs the SDK. The RTMP smoke test only accepts a loopback listener URL.

The operator window is Slint. Audience and stage windows show frames rendered by the `freeshow-output` crate and uploaded with wgpu. Those frames are not drawn as Slint widgets.

Settings and the log are stored in the app config directory:

- macOS: `~/Library/Application Support/freeshow-rust/`
- Windows: `%APPDATA%\freeshow-rust\`
- Linux: `$XDG_CONFIG_HOME/freeshow-rust` or `~/.config/freeshow-rust`

`FREESHOW_CONFIG_DIR` overrides that path. `settings.json` holds the theme name (when set), the FreeShow library path, and whether the output and stage windows open on launch. When no theme is set here, the library's own FreeShow theme is used. Custom theme colors and fonts are read from `Config/themes.json` using the theme ID selected in the library settings. The library `settings.json` also supplies `outLocked`, `lockedOverlays`, `splitLines`, and the other keys listed in `PARITY.md`; locked overlays stay on when a library opens, and `outLocked` blocks output changes. `splitLines` splits imported and edited lyric pages, `customFonts` files are used when a slide names that family, and `resized` left/right panel widths size the operator lists. The header Search box filters the current drawer tab. Missing or malformed values keep the built-in defaults. Font family and `em` font size apply to the operator window; unavailable fonts use the system fallback. `RUST_LOG` controls stderr tracing (default `info`).

## Layout

- `crates/freeshow` — control window and the output/stage windows
- `crates/freeshow-core` — theme colors, settings file, logging
- `crates/freeshow-data` — FreeShow `Shows/*.show` files and `Config/projects.json`
- `crates/freeshow-output` — headless RGBA compositor used by the live windows
- `crates/freeshow-media` — stills, cpal audio, and GStreamer video/camera frames
- `crates/freeshow-remote` — phone remote and stage web pages
- `crates/freeshow-osc` — OSC input
- `crates/freeshow-midi` — MIDI input
- `crates/freeshow-ndi` — NDI send of each enabled audience or stage picture. The show runs when the NDI plugin is missing
- `crates/freeshow-timecode` — MIDI timecode and linear timecode audio
- `crates/freeshow-record` — audience recording and RTMP. The show runs when the encoder is missing
- `crates/freeshow-pco` — Planning Center service plans. The show runs when no account is connected
- `crates/freeshow-churchapps` — ChurchApps presenter plans and Lessons.church media imports. The show runs when no account is connected
- `crates/freeshow-cloud` — library zip backup and an optional cloud copy. The show runs when the destination is missing
- `crates/freeshow-obs` — OBS WebSocket 5 scene, stream, and record control. The show runs when OBS is closed
- `crates/freeshow-ai` — slide rewrite through a local Ollama model. The show runs when no model is configured

See `PARITY.md` for what is still missing compared with FreeShow 1.6.8. See `docs/MIGRATION_PLAN.md` for the crate layout, UI-stack decision, and the remaining milestones.
