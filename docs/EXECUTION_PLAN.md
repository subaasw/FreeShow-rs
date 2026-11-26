# FreeShow-rs 1.0 execution plan

Use this document to order implementation and review. The [roadmap](ROADMAP.md) defines release scope. The [implementation plan](IMPLEMENTATION_PLAN.md) describes the intended product and architecture in more detail. Checkboxes in this file represent work still to be done unless marked complete.

## The plan in plain language

A volunteer should be able to open FreeShow, prepare a service, hand it to another volunteer, and run it with the internet disconnected. The editor should make it easy to create a polished slide. In the live room, everyone should know which slide the audience is seeing, and the operator should be able to recover from common problems without restarting the show.

| Step | What the volunteer can do when it is ready | Who demonstrates it |
| --- | --- | --- |
| 1. Protect the live show | Correct a slide backstage while the room keeps seeing the slide that was already on air. Publish the correction with **Update Live**. | An operator and a tester watch the editor, audience screen, and stage screen together. |
| 2. Trust editing and saving | Undo the right edit after switching songs. Recover a draft after a crash or a failed save. | A developer tests the failure paths. A tester repeats recovery in the app. |
| 3. Create polished slides | Build an announcement with a picture and text, align the parts, duplicate it, and adjust the copy. The controls work with a mouse or keyboard. | A volunteer creates a slide from a short brief and reports where they got stuck. |
| 4. Add motion and share the service | Preview transitions before the room sees them. Save an editable template and carry a complete service to a second computer. | A designer prepares the file. A second volunteer imports it and rehearses offline. |
| 5. Rehearse and release | Run a full service, reconnect a screen or phone when needed, and recover from a missing clip. | An operator runs the rehearsal. The release lead checks the installer and decides whether to ship. |

Display and media testing can start as soon as there is a repeatable sample service. The team can work on template discovery alongside the editor. The release sequence gives live safety and reliable saving priority because volunteers need both before relying on the new workflows live.

### Terms used in this plan

| Term | Meaning |
| --- | --- |
| Project | The service’s run order, which can contain songs, scripture, announcements, and media. |
| Cue | A step in the run order. The same slide can appear in more than one cue. |
| Preview | What the operator is reviewing before it goes live. |
| Program or live output | The picture the audience is seeing now. **Take** means send the selected cue to that output. |
| Stage view | The confidence monitor for people on stage. It can show notes, timers, and upcoming content. |
| Build | An animation for part of a slide, such as a second line appearing when the operator presses Next. |
| Release candidate | The packaged build used in the final rehearsal and install checks. |

Start with the [team rehearsal](#6-team-rehearsal-and-handoffs) if you are reviewing the experience or testing it in the app. Developers can start with the baseline and work package table below.

## 1. Start here

1. Record the checkout and worktree state. The planning review found local main nine commits behind its tracked upstream, with active changes to operator preview uploads in `main.rs` and `present.rs`. Compare both baselines in an isolated integration checkout when needed. Preserve the existing edits and verify the combined behavior before assigning new UI or renderer work.
2. Establish a repeatable baseline with the existing sample-library generator and fixtures in `freeshow-data/tests/fixtures`. Use two distinct shows, one arrangement that repeats a slide, a scripture cue, a still, a local video with audio, and a stage layout with notes. Keep a separate copy of a representative upgraded FreeShow library for compatibility testing.
3. Reproduce the two first-merge risks and record evidence. Edit a lyric and its notes while the slide is live, then force a repaint through a live timer, video frame, or stage refresh. For undo, edit show A, select show B, then undo and redo. Attempt the same operations with an injected save failure. Compare all rendered views, both document files, and undo/redo state.
4. Implement L1 and D1 as the first narrowly scoped changes. Use the regressions from step 3 to verify them. Begin display and media baselining in parallel.

The local planning preflight resolved all 16 Cargo workspace members offline and found GStreamer and its WebRTC SDK at 1.28.8 through pkg-config. Verify those prerequisites again on each implementation machine. Start with these individual commands:

```sh
git status --short --branch
cargo fmt --all -- --check
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo run --locked --release -p freeshow -- --check-library
cargo run --locked --release -p freeshow -- --check-service
cargo run --locked --release -p freeshow -- --check-perf
```

Run each check separately. The check modes can seed a sample library at a supplied path. Omit the path for the generated fixture or use a disposable copy. For framed QA, launch against a disposable library with `--data`, using both the default renderer and `FREESHOW_RENDERER=software` where supported. Keep new baseline failures in the review record so feature changes can be judged independently.

## 2. Work packages and merge order

IDs below are local references for reviews and dependencies.

| ID | Deliverable | Depends on | First code to inspect | Proof required |
| --- | --- | --- | --- | --- |
| B0 | Baseline and repeatable rehearsal fixture | — | `service_check.rs`, package workflows, `scripts/bench-perf.py` | Build SHA and renderer details, smoke outputs, Present/Edit captures, reproduction of current live-edit and undo behavior |
| L1 | Snapshot the taken content and isolate Edit | B0 | `session.rs` Go/Clear/reveal paths, `live.rs` program/stage paint, `main.rs` callbacks, remote snapshots | On-air lyric and notes stay at the taken revision while browsing, editing, saving, and repainting |
| D1 | Fix cross-show undo/redo and commit failure | B0 | `session.rs` history and item edit paths, `session/library_edit.rs`, `freeshow-data/src/lib.rs` atomic write | Redo captures the original target, deleted IDs fail safely, and failed saves preserve file, draft, and history state |
| D2 | Recover drafts and expose save state | D1 | Same mutation path, backup and autosave code, operator status bindings | Forced exit recovers a draft without overwriting a newer on-disk revision. Retry succeeds after the underlying problem is resolved |
| U1 | Present/Edit polish and accessible navigation | B0, L1 for live actions | `operator.slint`, `widgets.slint`, `panels.slint`, `main.rs` focus and shortcuts | Keyboard-only selection, Take, Clear, and recovery. Live and selected states remain distinct at compact size and high DPI |
| E1 | Layers, selection, arrange, and drag correctness | L1, D1 | Slint canvas/inspector, `item_edit.rs`, session mutation and preview path | Arrange overlapping items and undo one gesture in one step. Keyboard and click controls cover each drag operation |
| E2 | Text, image, and group editing | E1 | `item_edit.rs`, `live.rs` styled items, output text layout | Compose a reusable announcement with mixed styles and crop. Reopen and compare editor, audience, and reference FreeShow for supported features |
| T1 | Transition editor and isolated audition | L1 | `TextTransition`, `output_motion`, `motion_frame`, inspector | Choose, preview, apply to multiple slides, and reset to inherited without touching the live program |
| T2 | Transition interruption and media handoff | L1 | `fade_into`, `send_frames`, video opening fade, `FrameSlot` | Rapid Take and Clear during still/video transitions. Remote and local output end on the same intended cue |
| T3 | Object entrance/exit builds | E1, T1, T2 | Existing reveal and line-page state, compositor time sampling, remote cue readback | Verify chained and manual builds, Prev/Next, direct Go, Clear, and automatic advancement |
| P1 | Template browsing and reuse | B0, D1 for apply transactions | `choose_template`, `apply_template_id`, catalog and editor bindings | Use as New, Apply Style, and Save as Template are independently discoverable and undoable where they edit a document |
| P2 | Portable project/template package | P1, D2 for shared recovery rules | `freeshow-data/src/export.rs`, `import/community.rs`, media resolution | Clean-profile offline round trip, collision remapping, missing asset report, and safe cancelled or partial import |
| A1 | Offline media and relinking | B0, L1 for program interactions | `media_library.rs`, `church_import.rs`, `freeshow-media`, drawer bindings | Import, preview muted, relink, replay offline, and recover from missing files, cancellation, and disk errors |
| O1 | Output setup, rejoin, and preflight | B0, L1 | `main.rs` display/window paths, stage, remote, audio and network routes | Rehearse output reassignment, audio routing, stage, phone reconnect, and optional transport diagnostics on the platform matrix |
| R1 | Candidate qualification and release | L1 through O1 | Package workflows, rehearsal fixture, usage and release docs | Tagged candidate evidence, clean installation and migration, service rehearsals, blocker disposition, and tested rollback |

L1 and D1 can proceed in separate changes once B0 captures their shared fixtures. U1, P1, A1, and O1 can start in parallel where they have met their listed dependencies. Keep changes within the feature’s ownership boundary and coordinate edits to shared bindings in `main.rs` and `operator.slint`. Build the initial release around these packages. Evaluate shared-object transitions after T3, and keep additional ProPresenter fidelity and Spotify outside this queue.

## 3. First merge sequence

The first implementation adds a taken-slide snapshot for audience, stage, and remote output, and makes undo/redo writes transactional. Regression tests cover the main edit and retry cases. The installed-app rehearsal remains open. Completed show edits now use a recovery journal and can be retried, copied, or discarded.

### L1. Keep the live cue stable

- [ ] Map every path that takes, repaints, hides, or clears content. `ProgramContent::Slide` currently keeps IDs and reveal state. The held-slide painter and stage notes look those IDs up against mutable library data. `main.rs` also explicitly republishes edited live slides. Cover both direct repaint and later timer/video refresh in the regression.
- [ ] Move document capture to the point where the session accepts a Take. Preserve a snapshot of the taken slide data and the cue identity that selected it. Keep arrangement occurrence separate from slide ID for Next and project position. Reuse the same content in audience and stage views, and audit remote state against it. Capture the slide payload once per Take and continue sampling dynamic inputs through existing paths.
- [ ] Route explicit Update Live through the current live identity so changing the selection during confirmation cannot publish a different slide. Update the visual content and notes without restarting timers, audio, or media. Keep the live position and current reveal state, clamping reveal if edited content has fewer steps.
- [ ] Verify first Take, repeated Take, direct Go, automatic advance, scripture, action-only cues, reveal/line pages, output style, video, and Clear. An action-only cue that retains content must continue drawing the taken revision. Clear must win against work from the previous cue. Confirm saved click-to-go behavior in Present and clarify whether existing users need an explicit live-edit compatibility setting.

**Merge gate.** Take A, edit A’s lyrics and notes, select B, and drive ticks while A remains live. The editor shows the new revision, and audience, stage, and applicable remote views still show A’s taken revision. Return to A and Update Live. The new revision appears once without another cue action. Take B, clear during a transition, and verify subsequent ticks leave the output cleared.

### D1. Target the right document when undoing

- [ ] Resolve the pending history entry by show ID before capturing the inverse edit. Resolve slide selection by identity where possible. Remove restoration by an old list index. Reordering or deleting entries must never write into another show.
- [ ] Stage the replacement and use the existing file write path before moving the undo/redo entries. A failed undo or redo leaves the cursor, in-memory document, and file unchanged and surfaces the error. For a failed new edit, retain the user's input as a recoverable draft through D2.
- [ ] Extend the existing undo tests to check A → B → undo → redo, delete/reorder between edits, unsupported JSON fields, and a deterministic writer error. Compare actual JSON payloads, selected IDs, and history lengths.
- [ ] Add gesture coalescing with E1. Pointer updates alter a transient preview, release commits once, and Escape discards the gesture. Confirm a redo survives a failed commit and clears after a successful divergent edit.

**Merge gate.** Reopen both files after cross-show undo/redo and compare their content. While the destination writer fails, retry the same entry successfully after recovery. Verify that the live program stays on its taken revision throughout.

### D2. Make recovery visible and testable

- [ ] Track a saved revision and a dirty revision per document. If a write becomes asynchronous, use a single writer per destination and associate completion with its committed revision. Save failures keep the dirty indicator and actionable retry.
- [x] Compare the saved file with the draft’s base before writing. Save as Copy opens an independent show for review, and Discard reloads the saved version when available. Recovery files live in `Backups/Drafts`. Recovery starts when a committed edit begins saving. Uncommitted typing and dragging can still be lost.
- [x] Automated checks inject a write failure, reopen with the journal present, and verify retry, copy, discard, and protection for a newer external edit.
- [ ] Force-quit during an actual installed-app save and repeat the recovery walkthrough on the platform matrix.

## 4. Decisions with acceptance cases

### Editing and accessibility

Layers now supports additive selection, group movement and nudging, duplication, stacking changes, alignment, and distribution. Snap guides use the selection’s enclosing bounds and the slide or another layer’s edges and centers. A canvas drag saves once on release and checks that its original show, slide, and layer order still match. Lock and Hide now save with the slide and work across selections. Hidden layers stay available in the Layers list, while a taken slide keeps its published visibility until Update Live or another Take.

The Edit canvas now fits the slide’s authored aspect ratio and supports 100–400% zoom with bounded pan. Selection, snap guides, and resize handles follow the layer transform. A transformed resize holds the opposite corner in place, and Shift at drag start preserves proportions.

The installed-app display matrix remains open. Rehearse with an overlapping slide with a rotated picture and a slide with a custom aspect ratio. Verify selection, resize, one-step undo, and live output isolation at Fit and at 150% or higher, including on a scaled display.

Use the current Slint theme, spacing, and existing panels. Keep the center canvas dominant in Edit, with a single contextual toolbar and clear access to the layer list and inspector. For E1, prove a slide with overlapping text, picture, and shape can be edited by keyboard, by direct pointer manipulation, and by numeric fields. Hit testing and handles must agree at zoom-to-fit, crop/rotation, letterboxed viewports, and scaled displays. Focus returns to the edited item after a dialog closes. Test text shortcuts separately from live shortcuts.

E2 now saves groups that can be moved, aligned as a unit, resized proportionally from the canvas, duplicated, and reopened from templates. Resizing scales layer boxes and text sizes around the visible selection outline's opposite corner. The Rust editor stores membership on ordinary FreeShow layers, and the compositor keeps their existing order. The slide text editor preserves run styles around an edit and keeps paragraph metadata on retained lines. Regression fixtures cover mixed typography, a rotated layer, retained picture crop, save and reopen, one-step Undo, template reuse, and isolation from the live slide. Fixed pixel decoration such as shadows and borders keeps its authored values.

The slide text field now applies weight, size, and color to a selected range. This uses the same transactional save and Undo as other slide edits. The regression covers a backward Unicode selection across mixed runs, pending words, a paragraph break, and a collapsed selection. The Edit canvas shows the result while the field remains a plain text input.

1. Save an announcement with mixed text styles, rotated text, and a cropped picture. Reopen it in the Rust editor, reuse it as a template, and compare both results with the installed reference app for supported styles.
2. On a scaled display, resize and undo the group at Fit and 150%. Use a second display to verify that the audience keeps the taken version until **Update Live**.

### Motion and cue semantics

T1 now displays the effective transition from the slide, output style, or library. The inspector edits the effect, duration, direction, and easing using the FreeShow layout format. Range apply or reset is one undoable edit. The still-frame audition uses the same motion renderer and runs in a separate preview window with replay and scrubbing.

T2 now starts interrupted fades from the latest frame submitted to the output. This is also the fallback when a window is hidden or has yet to paint. A newer Take or Clear revokes the previous transition. Video and network frames update the incoming side of an active slide transition. Video Takes record their cue before decoding. The opening fade starts on the first decoded frame and advances through gaps in decoding using the held frame. The audience and operator preview share each submitted frame, and the stage clock updates the stage transition without overwriting it.

The automated rehearsal covers rapid Takes, Clear, video fade sampling, and stale frame uploads. The installed-app rehearsal still needs to compare physical, network, and stage outputs through the same still-to-video and video-to-still cues, including Clear mid-transition.

T3 has a Layer Animation inspector and a live build clock. Layers on the same numbered step enter in parallel. Step 0 starts on Take. Later steps can start On Click or After Previous. After Previous waits for the longest entrance on the prior step and does not wait for exits. Previous rewinds the last manual step and the automatic steps it started, after later text reveals have been rewound. A timed advance waits for scheduled automatic entrances. Layers can fade, move from and toward one of four directions, or scale around their center. Each exit begins after its entrance and configured hold. Returning to the previous slide shows its completed build state. A direct Take resets the steps. Clear cancels their frame updates. Built text follows the taken revision in stage and remote readback. Build order lists the playable layers and their shared triggers in step order. Selecting a row opens that layer's animation controls in Edit.

Tests drive parallel entrance, a manual second build, an automatic exit, editing while on air, Previous, retakes, and Clear. A timed destination waits while a manual step is pending. Ordinary advance countdowns are armed once all manual steps have been triggered. A manual step changes the current picture through the existing output path without starting a new slide transition, audio, or media action.

**Preview animations** now runs the selected slide's saved build sequence off air. The preview uses the same layer sampling and compositing as live output and caches local still artwork for the rehearsal. Operators can step by button or arrow key and restart from Take. The rehearsal test opens a second slide while the first remains live, then verifies the animation frames, cached artwork, and unchanged audience and remote readback. Installed rehearsal still needs to cover builds over still, video, and network backgrounds with local and network outputs enabled. Cross-slide shared-object effects remain open.

### Portable files and offline video

P1 separates template preview, Use as New, Apply Style, and named Save as Template. Use as New copies the design into an editable one-slide show. Apply Style keeps the open show’s words and uses the normal undo and recovery path. Template previews use saved slide proportions. Media continues to resolve within the current library. Favorites are saved per library, follow template IDs, and work with Search. The gallery includes original starters for lyrics, scripture, announcements, titles, and a service countdown. The countdown takes its duration from the editable digits, starts on Take, keeps elapsed time through Update Live, holds at zero, and stops on Clear or the next slide. Audience, stage, and remote readback use the taken countdown. Edit and gallery previews keep the authored duration.

P2 writes local project media using the published `data.json` and `files` layout. The Rust gallery can also export and import an editable template on its own. A template-only share removes private notes from its designs. Project shares retain stage notes. Export and import carry warnings for non-bundled named fonts and missing, external, or protected media. Import handles the hashed asset names used by the installed FreeShow 1.6.7 code. The import tests cover duplicate contents, basename collisions, shared show IDs, conflicting catalogs, Windows paths, and rollback after an injected project save failure. A clean Rust library can take a bundled picture and decode the bundled project video after the sender’s library is removed. The installed reference FreeShow check and cross-platform import rehearsal remain open. [FreeShow project format](https://freeshow.app/docs/format-project)

A1 previews local pictures and video off air and with no audio routes. The Media drawer finds and relinks missing files in the open show or current project. The replacement is stored locally and show edits use undo and recovery. **Download video** fetches an authorized direct file in the background, reports progress, and lets operators cancel or retry. A completed file enters the library after a local frame decode, with collision-safe naming and origin-only credit. Startup and retry clean abandoned staging files while a per-library lock protects active transfers. Disk-full errors tell the operator to free space and retry. A cross-platform rehearsal and a disk-full drill on a constrained volume remain open. YouTube downloading remains a separate research item. See the [media plan](IMPLEMENTATION_PLAN.md#4-media-output-and-integrations).

### Output setup and rejoin

O1 now displays the connected screen count and the OS-reported Audience and Stage window locations in the **Displays** check. Explicit audience assignments retain the display name and bounds when the OS reorders screens. An unavailable assignment or off-screen audience profile surfaces a persistent warning. Disconnecting the screen used by an open audience window closes that local window while the live cue and network outputs continue. Reopening is deliberate after reconnect. Stage can recover an off-screen placement on the operator display.

**Settings → Connection** reports actual bind success and failure per web listener. A remote-port conflict leaves a successfully bound Stage listener ready. The phone browser bounds stalled requests, disables controls while offline or locked, and reads the latest cue after reconnecting. An action with an unknown result is surfaced for operator confirmation. The Stage page retains its last frame through temporary failures and clears it if authorization is lost. The headless service check covers missing and invalid passwords and a fresh authenticated connection reading the current cue.

**Settings → Audio routing** scans audio-file and video speaker devices in the background. The check shows the source-to-speaker routes and saved mixer gains, distinguishes a network-only route from a missing local speaker, and reports unavailable audio-file channels. Device mappings save for the next launch so a setup change leaves live audio alone. For installed verification, route an audio file and a video clip to the intended mixer and ports, repeat after unplugging and reconnecting the interface, and check the network-only mix with local speakers off.

**Settings → Network outputs** reports NDI and OMT sender startup, WHIP negotiation, the selected RTMP destination, and Icecast mixer enablement. Grouped NDI stays closed on the current sink. The WHIP loopback test holds the server answer until it has observed the pending state. For the platform matrix, confirm each enabled transport with its intended receiver, then repeat with a missing plugin, a rejected endpoint, or a disconnected receiver as applicable. Record the behavior of both the sender and receiver.

For the installed O1 check, take the announcement with Audience and Stage visible. Unplug the audience display, confirm the local window closes and the stage and network cues continue, then reconnect and click **Output**. Verify the saved display and retained cue, then switch to a different screen and repeat in kiosk mode. Record the OS, display arrangement, and any window placement limitations. The physical display matrix, phone reconnect on a live Wi-Fi network, audio routes, and optional transport checks remain open.

## 5. Candidate sequence and sign-off

| Checkpoint | Run | Exit condition |
| --- | --- | --- |
| Integrated development | Own-crate tests and affected service smoke for each slice. Run workspace checks before combining public feature slices. | Regressions have a reproducer and a passing verification case. Document compatibility changes and known limits. |
| Feature complete | Test a clean library and an upgraded copy on the supported OS matrix. Build, edit, share, import, and run the same offline service. | Each required package passes its acceptance gate. Record unfinished stretch work separately. |
| Release candidate | Install from packaged artifacts, run the full-length rehearsal, compare the baseline on the same hardware, and run failure drills. Include both default and software rendering where supported. | No unresolved defect that loses data, publishes unintended content, exposes remote content, or prevents a supported service workflow. Changes after sign-off rerun the affected checks. |
| Public release | Verify installer signing and platform distribution status, check release notes and support matrix, and rehearse rollback from an upgraded library copy. | One record identifies the approved build, installer artifacts, test evidence, known limits, and rollback instructions. |

Capture each result with the work package ID, build SHA, OS and hardware, fixture revision, checks run, and a small log or screenshot for the behavior under review. Compare cold-start, input-to-preview, Take-to-visible, frame pacing, and memory with the [planned budgets](IMPLEMENTATION_PLAN.md#5-performance-and-release-verification). Use the same workload for before/after results.

`--check-service` reports a `portable_handoff` result from two disposable libraries. It exports a project and an editable template, removes the sender library, then takes the imported cues in order. It verifies distinct same-named pictures, a local video frame, Clear, relinking, saved template edits, and stage-note handling. The embedded two-second H.264/AAC fixture contains a synthetic 440 Hz tone. The check measures audio from the live player’s private output mix after import and after relinking. The installed rehearsal still covers physical speaker routing, displays, and the reference FreeShow import.

`--check-handoff` runs just the portable rehearsal without opening any HTTP listeners. The platform packagers record its result from the packaged launcher. A release candidate still needs evidence from the three native packaging jobs.

## 6. Team rehearsal and handoffs

### Prepare one service everyone can recognize

Use a disposable library with these five cues. Keep the same cue order and words across test runs so differences are easy to spot.

| Cue | Content | What it exercises |
| --- | --- | --- |
| 1 | Welcome slide with a logo and “Service begins at 10:00” | Template, layout, editing, and live correction |
| 2 | A song with a verse, chorus, and the same chorus repeated later | Run order, cue identity, undo across documents, and Next |
| 3 | A scripture slide with a note for the stage | Readability, stage, and the phone remote |
| 4 | An announcement where the date appears on the next click | Transition preview, object animation, and live cue handling |
| 5 | A short authorized local video with audio | Offline handoff, media relink, first frame, audio, and Clear |

Use a desktop or laptop as the operator machine and a second screen for audience output. Open the stage view and pair a phone. For the sharing check, use a second machine or a clean app profile. Prepare a second version of the library for upgrade and recovery tests. The exact content above is a test fixture, so the actions below describe expected 1.0 behavior.

### Operator walkthrough

| Test | Steps anyone can repeat | What counts as a pass |
| --- | --- | --- |
| A. Correct a slide while it is live | Take Welcome. In Edit, change 10:00 to 10:30 and change its stage note. Browse to the song while another person watches the screens. Return to Welcome and choose **Update Live**. | Before Update Live, the audience still reads 10:00 and the stage still shows the original note. After it, both show the correction. The app stays on the same cue and media or timers do not restart. |
| B. Undo after changing shows | Edit the chorus text. Select the Welcome show and choose Undo, then Redo. Close and reopen the library. | Undo and Redo affect the chorus. Welcome keeps its saved correction. The result survives reopen. |
| C. Build an announcement | Start from a template. Replace the sample text and picture, align the date with the title, and duplicate the slide. Repeat the arrange action with a keyboard or the inspector. | The original template remains reusable, the duplicate is editable, and one Undo reverses one arrange action. The layout matches on audience output. |
| D. Rehearse motion | Preview the transition into the announcement. Take it live and use Next to reveal the date. Run it again and press Clear during the transition. | Audition stays local. Next reveals the date before moving to the video. Clear blanks the output and the old animation cannot reappear. |
| E. Hand off the service | Export the project and an editable template. Import both in a clean profile using a machine offline. Play the clip, then make a test copy with the clip missing and relink it. | Cue order and editable text survive. The clip plays offline. Missing media is easy to identify and relink, and the receiving library keeps its existing projects. |
| F. Recover during use | On a disposable library, use the team’s save-failure test build or fixture. Retry a failed save. Reopen after a simulated crash with a draft. During rehearsal, disconnect and reconnect the second display and the phone. | The latest recoverable draft is available. A failed write leaves the previously saved file readable. The operator can see which output or connection needs attention and resume the service. |

Run the walkthrough once by keyboard for core authoring and show controls, and once with the pointer. Ask the tester to say which cue is on screen before pressing Take. If the interface gives them the wrong answer or provides no clear answer, record that as a live-control defect. Time the first attempt at authoring and handoff. Record where someone had to stop, guess, or ask for help.

### Who hands off what

- **Feature developer** creates the change, covers the relevant failure case, and hands over a build, sample library, and exact steps to exercise the change.
- **Design or accessibility reviewer** checks the controls in Present and Edit, confirms the keyboard and pointer paths, and reports where structure, labels, or focus made the task hard to complete.
- **Volunteer operator or tester** runs the walkthrough without coaching on the first pass and records the expected and observed outcomes. A second person can watch the audience and stage screens during live tests.
- **Release lead** checks the evidence against the candidate build, assigns unresolved issues, and records whether to ship or repeat the rehearsal. One person can cover several roles on a small team.

Use this short format when passing a result to the next person:

```text
Work package and walkthrough test
Build, operating system, and sample library
Steps taken
Expected result
Observed result
Evidence and impact on the service
Owner and next action
```

### Decide what stops the release

**Stop and fix** when a test loses or overwrites saved work, sends unintended content to the audience, exposes protected remote content, or prevents a supported service from continuing. Reproduce the failure, fix it, then repeat the affected check on the candidate build.

**Record and review** when a task is still achievable but takes extra steps, a layout looks crowded, or an optional device or provider needs a documented workaround. The release lead records the affected users, the workaround, and the follow-up owner before sign-off.

**Keep in the backlog** for additional ProPresenter fidelity and stretch motion work. Keep Spotify outside the release scope. The core video check uses an authorized local file while built-in YouTube downloading remains under investigation.
