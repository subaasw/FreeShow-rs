# Building and packaging

Release packaging for Linux, Windows and macOS, headless diagnostics, and optional transport checks. CI only runs these on release tags; see [RELEASING.md](RELEASING.md).

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

On a native Linux build host, `scripts/package.sh` also prepares a `.tar.gz` containing `AppRun`, the binary, relocated shared libraries, selected GStreamer plugins, the plugin scanner, desktop metadata, an icon, source/provenance, and third-party notices. It needs Python 3.11+, patchelf, binutils, desktop-file-utils, GStreamer development/runtime packages, and the usual X11/Wayland development libraries. The `.github/workflows/package-linux.yml` workflow lists the Ubuntu dependencies and is the native verification gate. Base glibc, graphics drivers, and desktop/audio services remain OS requirements; the measured minimum glibc version is recorded in `share/BUILD.json`. The optional NDI SDK remains separate.

Extract the Linux archive into a permanent folder, run `./AppRun` to launch directly, or `./install.sh` to add a per-user application-menu entry and `~/.local/bin/freeshow-rust`. `./install.sh --uninstall` removes that desktop integration and keeps your library. Moving the extracted folder later requires running its installer again. The desktop entry uses `/usr/bin/env` to launch the quoted application path, including folders containing a literal percent sign that GLib otherwise rejects during executable validation.

The Linux verifier checks relocation into a path with spaces, quotes, percent signs and Unicode; plugin discovery and H.264 decoding with an empty home/cache; the actual desktop-entry parser; and desktop removal without deleting a library sentinel. Native CI also removes system GStreamer packages and repeats decoding, then uploads the archive, checksum and reports as a workflow artifact. The **Release** workflow in `.github/workflows/release.yml` builds Linux x86-64, Windows x86-64, and macOS ARM64 packages from one Git revision and the checked-in Cargo lockfile. It runs only on `v*` tags and manual runs (see `RELEASING.md`). Each platform uses its native packager and uploads unsigned or ad-hoc signed artifacts to the run. All three packages passed native verification at revision `e075bea`. Use the steps above on a Mac with the owner's Apple credentials for Developer ID notarization. [Verified release run](https://github.com/subaasw/FreeShow-rs/actions/runs/45023447856)

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
