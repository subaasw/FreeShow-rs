# Plan

## Goal

A native Rust replacement for [FreeShow](https://freeshow.app/). The Electron
app (1.6.8) is the behavior reference. The service path is done when a user can
open an existing FreeShow library, edit a project, present songs and scripture,
run a stage layout, play media, and control the show from the remote page.

GPL-3.0-only.

## Where the project is

- The workspace builds and its tests pass.
- The service path works end to end: library files, shows, projects, scripture,
  stage layouts, media, output/stage windows, and the phone remote.
- `PARITY.md` is the detailed, feature-by-feature status. It is the source of
  truth for what is done, partial, or not-yet.
- The largest remaining gaps are network audio routing, deeper output
  and stage customization, importer media, and packaging.

## Priorities

1. **Correctness and trust** — a show must not lose data or expose the room's
   lyrics to anyone on the network.
2. **Service-path depth** — the features a volunteer touches every Sunday:
   audio routing, stage layouts, media, scripture.
3. **Compatibility** — read what FreeShow already wrote, including settings,
   themes, media metadata, and import formats.
4. **Packaging** — installers and reproducible release builds.

## Milestones

### M1. Library settings compatibility — complete

Read FreeShow's library `settings.json` instead of keeping a parallel world.

- [x] All eleven built-in FreeShow themes (`default`, `dark`, `light`,
      `white`, `black`, `blue`, `sea`, `aqua`, `charcoal`, `papyrus`,
      `terminal`).
- [x] Use the library's stored theme when this app has no explicit choice.
- [x] Read `serverData.remote.password` into the library preferences.
- [x] Read the remote/stage ports from the library settings.
- [x] Honor the theme selected in `settings.json`, including custom colors and
      font from `Config/themes.json`.

Acceptance: opening a FreeShow library shows its theme and keeps its remote
password without editing this app's config by hand.

### M2. Remote password and connection gating

FreeShow's `PASSWORD` / `ACCESS` handshake, enforced per connection.

- [x] `PASSWORD` reports whether a password is required.
- [x] `ACCESS` with the right password unlocks; a wrong password returns
      `ERROR { key: "wrongPass" }`.
- [x] Socket.IO, raw WebSocket, and the HTTP page share the same rule.
- [x] The built-in phone page prompts for the password and remembers it.
- [x] Stage page stays read-only and follows the same gate.

Acceptance: with a password set, no channel other than the handshake changes
the show or reveals lyrics until the password is accepted.

### M3. Per-output audio routing

- [x] Read `audioChannelsData`, outputs, and routing from `settings.json`.
- [x] Route drawer, slide audio, and playlists through their stored channels to speakers.
- [x] Route video sound through the active audience output's mixer channels.
- [x] Route individual hardware channels for audio files and video.
- [x] Feed the existing NDI and RTMP senders from saved stereo network routes.
- [ ] Remaining network transports (OMT, WebRTC, Icecast), NDI groups, and
      multiple independent network pictures.
- [x] Keep the main volume and mute controls working for paths through Main.
- [x] Remote channel list, per-channel volume, and independent mute.

Acceptance: a clip assigned to a secondary output plays only there, while the
main fader still controls the main channel.

### M4. Stage layout custom items

- [x] Every stage item type FreeShow writes (text, clock, timer, counter,
      variables, media, notes, next slide, and custom boxes).
- [x] Per-item style values already used by the compositor.
- [x] Layout switching from the stage window and from a slide action.

Acceptance: a `stage.json` made by FreeShow renders all of its boxes without
falling back to the four built-in types. QR boxes render from their saved
payload and colors. Live video, camera, web, and chart boxes render a labeled
placeholder until the compositor grows those sources.

### M5. Installers and packaging

- [x] Native macOS `.app`/`.dmg` with bundled runtime and documented signing.
- [ ] Verify macOS distribution with Developer ID/notarization and on additional OS/architecture hosts.
- [ ] Windows installer.
- [ ] Linux runtime tarball and desktop installer: implemented, native CI verification pending.
- [ ] Release workflow that builds all three from the lockfile.

Acceptance: a fresh machine can install the app without a Rust toolchain.

### M6. Importer media

- [x] ProPresenter 4–6 XML bundles and their slide-background media.
- [ ] ProPresenter 7 protobuf, arrangements, and original text-box styling.
- [x] Lessons.church OLP/OLF imports with downloaded, cached media.
- [x] ChurchApps service-plan lesson feeds and add-ons with downloadable media.
- [ ] Stream-only lesson media without a direct downloadable URL.
- [ ] Background lesson downloads with progress and cancellation.
- [x] ProPresenter bundle backgrounds land in `Media/` and stay linked.
- [ ] Background media from the other import formats.

Acceptance: importing a ProPresenter bundle produces shows that present with
their original backgrounds.

### M7. Remaining compositor and remote parity

Work down the `partial` rows in `PARITY.md`, highest user impact first. Each
item gets a test at the layer that owns it (`freeshow-output` for painting,
`freeshow-data` for storage, `freeshow-remote` for protocol).

## Working method

- Small, coherent commits; each one leaves `cargo test --workspace` green.
- New behavior gets a test in the crate that owns it.
- Update `PARITY.md` and `README.md` in the same commit as the behavior.
- Push after each coherent milestone so the remote is never far behind.
- When FreeShow behavior is ambiguous, read the Electron source in `../FreeShow`
  rather than guessing.
