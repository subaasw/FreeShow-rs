# Releasing

GitHub Actions runs **only** for releases. Pushing to `main` or opening a pull
request starts nothing, so day-to-day work never spends Actions minutes. Run the
local gate before every commit instead:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

## Workflows

| File | Trigger | What it does |
| --- | --- | --- |
| `release.yml` | push of a `v*` tag, or **Run workflow** | The only entry point. Checks the tag matches `Cargo.toml`, runs the gate, packages all three platforms, then publishes the release (tags only). |
| `gate.yml` | called by `release.yml`, or **Run workflow** | `rustfmt` once, `clippy -D warnings` on Linux, Windows and macOS. |
| `package-linux.yml` | called by `release.yml` | Tests, builds and verifies the x86-64 `.tar.gz` (relocated GStreamer runtime) and runs `--check-perf` on the packaged AppRun. |
| `package-windows.yml` | called by `release.yml` | Tests, builds the MSVC release, packages and verifies the Inno Setup `.exe` without the GStreamer SDK. |
| `package-macos.yml` | called by `release.yml` | Tests, builds and verifies the ad-hoc signed ARM64 `.app`/`.dmg`. |

Each package job writes a `<asset>.sha256` sidecar (`<hex>  <file name>`). The
publish job re-checks every sidecar with `sha256sum --check` and uploads the
installers and sidecars to the GitHub release. The in-app updater
(`crates/freeshow/src/update.rs`) reads `releases/latest`, picks the asset for
the running OS and architecture, downloads its `.sha256` sidecar and refuses
to install on a mismatch.

## Cutting a release

1. Bump `version` under `[workspace.package]` in `Cargo.toml`, run
   `cargo check` so `Cargo.lock` follows, and commit
   (`chore: release v0.2.0`).
2. Run the local gate and `cargo run -p freeshow --release -- --check-perf`.
3. Tag and push the tag:

   ```sh
   git tag -a v0.2.0 -m "FreeShow-rs v0.2.0"
   git push origin v0.2.0
   ```

4. Watch **Actions → Release**. When it finishes the release appears under
   **Releases** with notes generated from the commits since the previous tag.

A tag that does not match `Cargo.toml` fails in the first job, before any
runner time is spent on builds.

## Trying the packages without releasing

**Actions → Release → Run workflow** on any branch runs the gate and all three
package jobs and keeps the installers as run artifacts for 14 days. No release
is created. **Actions → Gate → Run workflow** runs only fmt and clippy.

## Signing

Release builds are unsigned on Windows and Linux and ad-hoc signed on macOS.
Developer ID signing and notarization need the owner's Apple credentials; the
manual steps are in [`PACKAGING.md`](PACKAGING.md).
