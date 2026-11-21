# freeshow-rust

Native Rust rewrite of [FreeShow](https://freeshow.app/). The Electron app stays the behavior reference until this project can run a service on its own.

GPL-3.0-only.

## Run

Install Rust 1.92 or newer and GStreamer 1.x (macOS: `brew install gstreamer`). On Apple Silicon, point pkg-config at Homebrew before building: `export PKG_CONFIG_PATH="$(brew --prefix)/lib/pkgconfig"`. Video and cameras use GStreamer. Audio uses the system output device. Then from this directory:

```sh
cargo run -p freeshow
```

Point it at an existing FreeShow library, or open one show:

```sh
cargo run -p freeshow -- --data ~/Documents/FreeShow
cargo run -p freeshow -- --show ~/Documents/FreeShow/Shows/Some\ Song.show
```

`scripts/present-sample.sh` builds a one-song library and opens the first slide on the output window.

Put song sources in the library `Imports/` folder before opening the library. ProPresenter 4–6 XML (`.pro4`, `.pro5`, `.pro6`) and `.proBundle` archives import lyrics, group children, disabled slides, notes, and slide backgrounds. Bundled media is copied into `Media/` and the saved shows point there; existing files are kept, and matching media is reused on repeat imports. Missing or ambiguous bundle media produces an import warning instead of linking to another file. ProPresenter 7 `.pro` protobuf, arrangements, and original text-box styling are not implemented. Bundles are limited to 10,000 entries, 16 MiB per XML document, 512 MiB per media file, and 2 GiB uncompressed in total.

While the app is running, a phone on the same network can open `http://<this-computer>:5510/` to change slides. A stage page is at `http://<this-computer>:5511/`. The ports come from the library `settings.json` when it sets `ports.remote` and `ports.stage`. Both speak FreeShow's message shape, `{ "channel", "data" }`, on a WebSocket at `/ws` and on the Socket.IO events `REMOTE` (remote port) and `STAGE` (stage port). When the library `settings.json` has `serverData.remote.password`, both pages ask for it and every socket connection must send `ACCESS` with that password before any channel is answered. The password is sent as the `x-freeshow-password` header on the page's HTTP requests. Channels that move the show are `API:next_slide`, `API:previous_slide`, `API:index_select_slide`, `API:clear_all`, `SHOW`, and `API:start_scripture`. `GET_SCRIPTURE` and `SEARCH_SCRIPTURE` read the Bibles in the library. `API:scripture_next` moves to the next verse. On connect the remote also receives `PROJECTS`, `FOLDERS`, and `PROJECT`. `SEARCH_SHOWS` finds a song, and `API:set_plain_text` saves lyric slides separated by a blank line. `API:create_project`, `API:rename_project`, `API:delete_project`, and `API:remove_project_item` edit the service list. `API:get_groups` lists verse and chorus groups. `API:change_layout` switches a song to another layout. `GET_TEMPLATES` and `API:set_template` list templates and apply one to the open song. `API:id_start_timer`, `API:id_pause_timer`, and `API:id_stop_timer` run a countdown from the library. `API:id_select_overlay` draws an overlay on the audience. `API:play_media` plays a file from the library, `API:pause_audio` holds it, `API:audio_seekto` and `API:video_seekto` move the playhead, `API:toggle_media_loop` turns background looping on or off, and the volume slider on the phone page sets the main output level.

Audio files and video sound follow the library's `audioRouting.connections` graph through `audioChannelsData` mixer gains to the default speaker or named speaker devices. Slide audio and drawer audio use `drawer_audio`; playlists use both `playlist_sub_<id>` and `playlists_default`. A source with no speaker connection stays silent. The phone page lists mixer channels and lets you set each channel's volume and mute independently. Main affects paths through the Main channel. Video sound uses `output_window` and `output_win_sub_<audience-output-id>` and keeps the clip's stored volume.

FreeShow's named speaker IDs come from Chromium and need a native device mapping. Run `cargo run -p freeshow -- --list-audio-devices` for audio-file devices or `cargo run -p freeshow -- --list-video-audio-devices` for video speaker devices, then add the exact device name to this app's `settings.json`, for example:

```json
{"audio_devices": {"speaker_sub_<FreeShow-device-id>": "Stage Speakers"}}
```

Keep the other app settings when adding that field. An unavailable mapped device logs an error and does not fall back to the room speakers. Audio-file speaker devices and network feeds use independent playback clocks. Video sound uses GStreamer speaker sinks on its playback clock. Hardware speaker connections honor `channelIndex`/`toChannelIndex` and an explicit `fromChannelIndex`. A single selected speaker port receives a mono mix; multiple selected ports preserve the corresponding source channels. Named speaker connections without an index use port 0, as FreeShow does. Audio and video share the same channel matrix, and unmapped ports stay silent. Indices are zero-based and must be below 64; unavailable device channels fail without falling back to the room speakers. Stereo network feeds follow `network_sub_<output-id>` routes and keep audio-file and video sources separate until the sender mixes them. A network-only audio file needs no physical output device. OMT, WebRTC, Icecast, effects, and delays are not implemented.

OSC input listens on UDP port 5505, the same port FreeShow uses. `/freeshow/slide/next`, `/freeshow/slide/previous`, `/freeshow/slide/1`, `/freeshow/clear/all`, and `/freeshow/show/<id>/start` drive the open show.

MIDI input opens the first connected input, or the port named by `FREESHOW_MIDI_IN`. FreeShow's default notes are used: channel 1 note 0 is next, note 1 is previous, channel 3 note 0 clears, and channel 5 note 2 selects a slide from the velocity.

The first enabled audience NDI output in the library sends the current audience picture and its routed stereo audio through the GStreamer NDI plugin. Its source name uses `ndiData.name`, or `FreeShow NDI - <output name>`. With no enabled NDI output, nothing is broadcast. The plugin and NDI SDK runtime are required. Grouped outputs stay disabled because the current sink does not expose NDI discovery groups. Multiple independent NDI pictures and stage NDI are not implemented.

MIDI timecode uses the first input, or the port named by `FREESHOW_MTC_IN`. Quarter-frames and full frames replace the stage clock with `HH:MM:SS:FF` while they are arriving. Linear timecode listens only when `FREESHOW_LTC_IN` names an input (`default` uses the system input) and uses 25 fps unless `FREESHOW_LTC_FPS` is set.

Functions → Record writes the audience picture to `Recordings/freeshow-YYYYMMDD-HHMMSS.mp4` in the library. Choosing it again, or closing the app, closes the file. Local MP4 recording is still video only. `FREESHOW_RTMP` streams the current picture with routed stereo AAC audio to that URL. It uses the first enabled RTMP output ID in the library, or `default` when none is configured; route to `network_sub_<id>` to feed it. Saved `rtmpData.destinations` are not started automatically. Static pictures repeat at 30 fps so media audio can continue without slide changes. Network feeds require the GStreamer `inter` plugin; RTMP also requires an AAC encoder (`avenc_aac`).

Planning Center loads when `FREESHOW_PCO_TOKEN` is a Services access token and `FREESHOW_PCO_PLAN` is `serviceTypeId/planId`. `FREESHOW_PCO_FILE` can point at a saved plan document instead. Songs and plan items become shows, headers stay section labels in the project, and Functions → Planning Center loads the plan again. With neither a token nor a file, the app does not contact Planning Center.

ChurchApps loads the presenter plans for `FREESHOW_CHURCHAPPS_TOKEN`, or a saved plan from `FREESHOW_CHURCHAPPS_FILE`. Arrangement lyrics become shows, with later lyric blocks kept as child slides, and Functions → ChurchApps loads them again. With neither a token nor a file, the app does not contact ChurchApps.

Functions → Backup writes `Backups/YYYY-MM-DD_HH-MM.zip` with shows, portable config, and bibles in the FreeShow backup names. Functions → Restore puts the newest of those zips back into the library. `FREESHOW_CLOUD_DIR` also receives a copy, and `FREESHOW_CLOUD_URL` receives a PUT of the zip (`FREESHOW_CLOUD_TOKEN` is sent as a bearer token when it is set). A failed copy or upload keeps the local zip and the show continues. When neither destination is set, the app does not upload on startup.

OBS uses WebSocket 5. `FREESHOW_OBS` is `host:port` (port 4455 when the host has none) and `FREESHOW_OBS_PASSWORD` is the server password. Functions → OBS scene sends `SetCurrentProgramScene` for `FREESHOW_OBS_SCENE`. Functions → OBS stream and OBS record start and then stop that output. When `FREESHOW_OBS` and `FREESHOW_OBS_SCENE` are both set, the scene is selected at startup. A closed OBS is logged and the show continues.

Functions → AI rewrites the selected slide through Ollama when `FREESHOW_AI` is `ollama` or a base URL such as `http://127.0.0.1:11434`. `FREESHOW_AI_MODEL` defaults to `gemma3:4b`, and `FREESHOW_AI_PROMPT` is the instruction. The model replies with slide lines, which replace the selected slide. With `FREESHOW_AI` unset, the app does not contact a model.

Every 15 minutes the open shows and the project file are saved again. A file whose bytes did not change is left alone, so the `Backups` folder does not grow. Each save still writes a temporary file, flushes it, and renames it into place.

## Release build

`scripts/package.sh` builds the locked release binary and stages `dist/freeshow-<target>/` with the binary, `LICENSE`, `README.md`, and `BUILD.txt` (compiler version, `SOURCE_DATE_EPOCH`, and the Cargo.lock hash). Paths inside the binary are remapped to `/freeshow`.

```sh
scripts/package.sh
scripts/package.sh x86_64-pc-windows-msvc
scripts/package.sh x86_64-unknown-linux-gnu
```

The extra arguments need that Rust target installed, plus a GStreamer development package for the platform. The script does not build disk images or installers.

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

`FREESHOW_CONFIG_DIR` overrides that path. `settings.json` holds the theme name (when set), the FreeShow library path, and whether the output and stage windows open on launch. When no theme is set here, the library's own FreeShow theme is used. Custom theme colors and fonts are read from `Config/themes.json` using the theme ID selected in the library settings. Missing or malformed values keep the built-in defaults. Font family and `em` font size apply to the operator window; unavailable fonts use the system fallback. `RUST_LOG` controls stderr tracing (default `info`).

## Layout

- `crates/freeshow` — control window and the output/stage windows
- `crates/freeshow-core` — theme colors, settings file, logging
- `crates/freeshow-data` — FreeShow `Shows/*.show` files and `Config/projects.json`
- `crates/freeshow-output` — headless RGBA compositor used by the live windows
- `crates/freeshow-media` — stills, cpal audio, and GStreamer video/camera frames
- `crates/freeshow-remote` — phone remote and stage web pages
- `crates/freeshow-osc` — OSC input
- `crates/freeshow-midi` — MIDI input
- `crates/freeshow-ndi` — NDI send of the audience picture as `FreeShow NDI`. The show runs when the NDI plugin is missing
- `crates/freeshow-timecode` — MIDI timecode and linear timecode audio
- `crates/freeshow-record` — audience recording and RTMP. The show runs when the encoder is missing
- `crates/freeshow-pco` — Planning Center service plans. The show runs when no account is connected
- `crates/freeshow-churchapps` — ChurchApps presenter plans. The show runs when no account is connected
- `crates/freeshow-cloud` — library zip backup and an optional cloud copy. The show runs when the destination is missing
- `crates/freeshow-obs` — OBS WebSocket 5 scene, stream, and record control. The show runs when OBS is closed
- `crates/freeshow-ai` — slide rewrite through a local Ollama model. The show runs when no model is configured

See `PARITY.md` for what is still missing compared with FreeShow 1.6.8.
