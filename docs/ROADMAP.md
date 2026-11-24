# Roadmap

## Goal

A native Rust replacement for [FreeShow](https://freeshow.app/). The Electron
app (1.6.8) is the behavior reference. The service path is done when a user can
open an existing FreeShow library, edit a project, present songs and scripture,
run a stage layout, play media, and control the show from the remote page.

GPL-3.0-only.

## Architecture

The crate layout, UI-stack decision, performance budgets, and the remaining
milestones to feature parity are in `MIGRATION_PLAN.md`. This file stays
the short working plan. Do not replace Slint or the headless compositor.

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
- [x] Icecast stereo audio on the saved mixer node with operator and slide controls. Plain HTTP and certificate-verified TLS work without libshout.
- [x] Separate audience and stage pictures over NDI and WebRTC/WHIP.
- [x] Route each WebRTC/WHIP output's stereo mix into its Opus track.
- [x] Send audience and stage video and routed stereo over OMT from the Rust app.
- [x] Receive configured OMT video backgrounds with the Rust SDK.
- [ ] Verify OMT with third-party implementations, add NDI groups, and support
      multiple independent audience pictures.
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
payload and colors. Current Output boxes mirror the active audience picture, including live media. Local stage videos play silently, and slide-text boxes can mirror audience backgrounds. The native stage also supports camera sources, website previews, and chart boxes.

### M5. Installers and packaging

- [x] Native macOS `.app`/`.dmg` with bundled runtime and documented signing.
- [x] Apple Silicon disk image verified in native macOS 26 CI.
- [ ] Verify Developer ID signing, notarization, and compatibility on additional macOS versions.
- [x] Windows x86-64 installer verified in native CI, including install, H.264 playback without the build SDK, and uninstall.
- [x] Linux runtime tarball and desktop installer verified on Ubuntu 24.04 x86-64.
- [x] Verify the three-platform release workflow against one revision and its lockfile. All three native jobs passed and uploaded installers for `e075bea`. [Release run](https://github.com/subaasw/FreeShow-rs/actions/runs/45023447856)

Acceptance: a fresh machine can install the app without a Rust toolchain.

### M6. Importer media

- [x] ProPresenter 4–6 XML bundles and their slide-background media.
- [x] ProPresenter 7 protobuf, arrangements, original text-box geometry, and box-level styling.
- [x] ProPresenter 7 mixed RTF fonts, colors, emphasis, paragraph alignment, all caps, and letter spacing.
- [x] ProPresenter RTF hex escapes decoded with their document code page and font charset.
- [x] ProPresenter RTF typographic punctuation and spacing, including Unicode fallback encoded as control words.
- [x] ProPresenter 7 RTF highlights and character background colors appear behind their text runs in native output.
- [x] ProPresenter 7 text boxes preserve saved margins, glyph outlines, and text drop shadows.
- [x] Hidden RTF text omitted from imported ProPresenter lyrics and notes.
- [x] ProPresenter RTF raised and lowered runs, including superscript and subscript.
- [x] ProPresenter 7 solid rectangle and ellipse fills with stored rotation rendered in Rust.
- [x] ProPresenter 7 triangle and rhombus fills follow the shape while text uses the rectangular text box.
- [x] ProPresenter 7 regular polygons and stars use the stored side or point count and the star's inner radius.
- [x] ProPresenter 7 linear and centered radial shape fills preserve gradient stop colors, transparency, and positions.
- [x] ProPresenter 7 Background Blur and Color Invert fills use the saved shape for their effect on the underlying picture.
- [x] ProPresenter 7 timed next-slide, loop-to-start, and specific-cue jumps within an arrangement, including slides with background media.
- [x] ProPresenter 7 enabled, immediate global clear-audio and clear-background cues. Standalone cues retain the content already on audience and stage output.
- [x] ProPresenter 7 timeline slide cues play at their recorded times in the imported arrangement.
- [x] ProPresenter 7 global clear-audio and clear-background actions fire at their recorded timeline times.
- [x] ProPresenter 7 timeline background image and video cues play independently of slide selection, with video loop and retrigger settings.
- [x] ProPresenter 7 foreground pictures and videos import as foreground slide actions or timed timeline cues and clear when the next slide is triggered.
- [x] ProPresenter 7 timeline soundtracks use linked or bundled audio and follow timeline playback and looping.
- [x] ProPresenter 7 timeline media and clear cues honor saved action delays. Soundtrack delays apply on each repetition.
- [x] ProPresenter 7 global clear-slide cues clear the slide layer in standalone cues and on timelines.
- [x] ProPresenter 7 global Clear All cues use native output clearing and stop both audio and video.
- [x] ProPresenter 7 audio actions use native playback as slide cues or timeline events, with standard loop and retrigger settings.
- [x] ProPresenter 7 slide-triggered media and global clear actions honor saved delays.
- [x] ProPresenter 7 videos honor hold, stop on black or clear, and fade to black or clear.
- [x] ProPresenter 7 slide and timeline videos honor saved in and out points and playback rate, even when cues share a file.
- [x] ProPresenter 7 slide audio, timeline audio cues, and timeline soundtracks honor saved in and out points and playback rate.
- [x] ProPresenter 7 timeline looping follows the imported duration and survives edits to the arrangement timeline.
- [ ] Other ProPresenter 7 graphics, remaining RTF controls, and other timeline tracks and cue actions.
- [x] Lessons.church OLP/OLF imports with downloaded, cached media.
- [x] ChurchApps service-plan lesson feeds and add-ons with downloadable media.
- [x] Import stream-only lesson media by URL and resolve YouTube or Vimeo for playback when `yt-dlp` is installed.
- [x] Background lesson downloads with progress and cancellation.
- [x] ProPresenter bundle backgrounds land in `Media/` and stay linked.
- [x] PowerPoint embedded slide, layout, and master backgrounds.
- [x] Solid PowerPoint theme fills and slide-level color-map overrides.
- [x] PowerPoint solid-fill tint, shade, and luminance transforms.
- [x] PowerPoint linear and centered circular gradients, linked backgrounds, and tiled media.
- [x] PowerPoint background alpha and gamma color transforms.
- [x] PowerPoint circular gradients with an off-center point focus.
- [x] PowerPoint rectangular gradients with a focus inside the slide.
- [x] PowerPoint clockwise linear gradient angles and aspect-ratio scaling.
- [x] PowerPoint gradient tile rectangles with mirrored repeats and signed insets.
- [x] PowerPoint fractional gradient stops, explicit empty and single-stop fills, and gamma for eligible two- and three-stop gradients.
- [x] PowerPoint shape-path slide backgrounds, default center focuses, and radial ellipse focuses.
- [x] Direct and theme gradient fills for positioned PowerPoint shapes and text boxes.
- [x] PowerPoint custom polygons with straight edges retain their outlines, including filled subpaths without explicit closing commands. Point-focus shape-path gradients follow convex outlines and concave outlines when the focus can see the full boundary.
- [x] PowerPoint triangle, right-triangle, diamond, and 4- to 32-point star presets retain their outlines and saved numeric adjustments.
- [x] PowerPoint block arrows, chevrons, pentagonal arrows, parallelograms, trapezoids, hexagons, octagons, and plus shapes retain their saved adjustments and short-side scaling.
- [x] PowerPoint rounded and snipped rectangle presets preserve their saved corner dimensions.
- [x] PowerPoint pie, pie wedge, chord, arc, and donut preset fills retain curved outlines and donut cutouts.
- [x] PowerPoint block arcs and prohibition symbols retain their adjusted bands and cutouts.
- [x] PowerPoint quadratic and cubic Bézier segments in closed custom outlines follow their saved control points.
- [x] PowerPoint elliptical arcs in closed custom outlines keep their radii, start angles, and signed sweeps.
- [x] PowerPoint custom path points and arcs resolve DrawingML guides in their effective coordinate system.
- [x] PowerPoint compound contours preserve cutouts, and independent paths paint in document order with their saved lightening and darkening.
- [x] PowerPoint cube, bevel, folded corner, and cylinder presets retain their adjusted faces and separate shades.
- [x] PowerPoint object rotation, picture flips, and gradient fill reflection.
- [x] PowerPoint straight connectors and line shapes preserve solid stroke width, color, direction, preset and custom dash patterns, end caps, end markers, compound bands, and theme line styles.
- [x] Nested PowerPoint group transforms, including child offsets, nonuniform scaling, rotation, and flips.
- [x] PowerPoint gradients that stay aligned to the slide while their shapes rotate.
- [x] PowerPoint positioned text with mixed run styles, theme fonts and colors, paragraph alignment, and explicit line breaks.
- [x] PowerPoint positioned text preserves saved character baseline offsets and inherits paragraph defaults.
- [x] PowerPoint text-box insets for wrapping and alignment inside positioned shapes.
- [x] PowerPoint text boxes with wrapping disabled keep their explicit line breaks.
- [x] PowerPoint saved normal autofit font scaling and percentage line-spacing reduction.
- [x] PowerPoint speaker notes in the FreeShow stage view, linked to their source slides.
- [x] PowerPoint saved automatic advance timings and all-slides loops.
- [x] PowerPoint named custom shows and saved slide ranges as arrangements, with local automatic loops.
- [x] PowerPoint section names as FreeShow slide groups across all arrangements.
- [x] PowerPoint cuts, fades, fades through black, and directional pushes with saved transition durations.
- [x] PowerPoint directional wipes, covers, and uncovers, including corner motion for cover and uncover.
- [x] PowerPoint embedded transition sounds with saved loop and stop cues.
- [x] PowerPoint line spacing and paragraph spacing before and after, including wrapped text and hard breaks.
- [x] PowerPoint paragraph margins and first-line or hanging indents, including wrapped continuations.
- [x] PowerPoint character bullets with inherited font, size, and color and one marker per paragraph.
- [x] PowerPoint decimal, Latin alphabetic, and Roman numbering with saved starts and nested list counters.
- [ ] PowerPoint custom gradient geometry and backgrounds from other import formats.

Acceptance: importing a ProPresenter bundle produces shows that present with
their original backgrounds.

### M7. Remaining compositor and remote parity

Work down the `partial` rows in `PARITY.md`, highest user impact first. Each
item gets a test at the layer that owns it (`freeshow-output` for painting,
`freeshow-data` for storage, `freeshow-remote` for protocol).

## Working method

- Small, coherent commits; each one leaves `cargo test --workspace` green.
- New behavior gets a test in the crate that owns it.
- Update `PARITY.md` and `USAGE.md` in the same commit as the behavior.
- Push after each coherent milestone so the remote is never far behind.
- When FreeShow behavior is ambiguous, read the Electron source in `../FreeShow`
  rather than guessing.
