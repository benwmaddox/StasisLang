# Stasis Android Workshop Shell

This is the first checked-in Android app shell for the `android` branch.

Current scope:

- Builds one Android app module with `workshop` and `published` product flavors.
- Targets `arm64-v8a` only.
- Loads a tiny native C library through JNI.
- Bundles a small Stasis-style workshop project under Android assets.
- Opens the native Android symbol browser and source editor from a top-right hamburger overlay grouped by Main, Structs, Systems, and Root.
- Opens into a full-screen native Android preview surface by default and starts an automatic 60 fps compile/tick loop so the preview advances without pressing `Compile` and `Run Tick`.
- Keeps the preview, status text, and menu button inside Android system-bar and display-cutout safe insets, so they do not sit under the bottom navigation bar or camera notch.
- Seeds bundled `.stasis` files into app-private storage when missing and preserves edits across app launches.
- Lets a selected symbol display and edit its source from the app-private `.stasis` file.
- Saves selected symbol edits back to the app-private `.stasis` file, reparses the project so later edits use fresh symbol spans, and reports `FastReload` versus `ResetRequired` expectations.
- Provides a dev-first AI edit panel that stores an OpenAI API key/model in app-private preferences, sends the selected symbol context to the Responses API, applies supported `replace_function`/`replace_struct` JSON edits, and refreshes compile state.
- Provides an explicit `Reset Project` control for restoring the bundled sample. Manual `Apply` saves and compiles immediately; there is no separate manual compile button. The automatic loop and manual controls both use the native compile/run path, with runtime ticks routed through the Rust JIT bridge when packaged. The probe reads project `.stasis` files, validates basic source structure, checks lifecycle roots, and writes `build/native_compile_manifest.txt` with project counts, per-function signature/body hashes, per-function compiled-stub artifacts under `build/functions`, a `build/runtime_state.txt` state artifact, and a reload classification (`InitialCompile`, `NoChange`, `FastReload`, or `ResetRequired`), then returns `CompilePlanned` or `CompileError` diagnostics.
- Resizes and scrolls the editor when the Android keyboard opens so the active source remains visible.
- Keeps fixed trailing scroll space under the editor as a fallback for phones where IME resize is inconsistent.

It packages the tested Rust/C ABI bridge from `crates/stasis_android_bridge`, runs the bundled Stasis game through the native compile/run path, and keeps compiler-owned Android compile plan/artifact rendering in `stasis_compiler::frontend::workshop`. The C/JNI scaffold remains as the Android host boundary and fallback layer while the bridge evolves toward the full workshop runtime.

## Published Build Path

The `workshop` flavor is the general-purpose `com.stasislang.workshop` product. It can open arbitrary projects and may preload several templates; it is never branded as one game.

The current `published` flavor is the game-specific Pong release, labeled `Stasis Pong` with application id `com.stasislang.pong`. It keeps the same native rendering/runtime surface but skips the workshop chrome entirely: no hamburger drawer, symbol browser, source editor, AI controls, or manual hot-swap UI. It can be installed next to the general Workshop. The APK links the Pong arm64 AOT objects directly and excludes Stasis source, tests, compiler stubs, unrelated templates, and the Rust JIT bridge. Future releases must declare their own game ID/project/package rather than turning this flavor back into a generic game container.

Build the release-style APK with:

```powershell
.\build_published.ps1
```

Install a debug-signed published variant for device testing with:

```powershell
.\build_published.ps1 -Install
```

The build script runs an APK-content gate after Gradle succeeds. `-ValidateAot` additionally runs the compiler AOT bundle contract test before building. A release build is unsigned until a release signing configuration is supplied; `-Install` uses the debug-signed published variant for device acceptance.
## Build

Prerequisites:

- Android SDK
- Android NDK
- JDK 17+
- Gradle or Android Studio

From this directory:

```powershell
.\build_debug.ps1
```

Or call Gradle directly:

```powershell
gradle :app:assembleWorkshopDebug
```

If your installed SDK is not 35, override it:

```powershell
gradle :app:assembleWorkshopDebug -Pstasis.compileSdk=36 -Pstasis.targetSdk=36
```

Install to a connected device:

```powershell
.\build_debug.ps1 -Install
```

Record a bounded device acceptance launch for the workshop APK:

```powershell
.\validate_device.ps1 -Install -RequireDevice
```

Use `-Published` for the published debug package. Without `-RequireDevice`, an unavailable phone/emulator writes an explicit skipped JSON record under `artifacts/android_device_acceptance/` and exits successfully; CI or a release gate should use `-RequireDevice`.

## Host AI Run Review

Validate the Android AI Responses payload locally before a live run. This does not call OpenAI or modify the bundled sample, and writes a timestamped trace under the ignored `artifacts/android_ai_runs/` directory:

```powershell
python ..\..\tools\android_ai_agent_host.py --preflight --prompt "verify Android AI payload"
```

For a live local run, the same host tool always writes a trace, including API failures, tool observations, test results, token usage, and cost estimates. After it finishes, provide the printed trace path for review:

```powershell
python ..\..\tools\android_ai_agent_host.py --reset-paddle-speed-feature --prompt "enemy paddle should have speed change"
```

Expected app surface:

```text
tick=<avg ms> render=<avg ms> budget=<tick+render % of 60 fps frame>
[full-screen native preview]
[top-right menu button]
```

Open the top-right menu button to access the AI prompt first, with manual symbols/source collapsed below it. Manual Apply/Reset, Changes, Reset Project, and Run Tick live with the selected manual symbol editor; Apply compiles immediately.

GitHub settings remain collapsed below the command workflow. After configuring a token, `owner/repository`, and base branch, `Sync GitHub Now` uploads changed project files directly to that branch. For reviewed work, use `Review GitHub Changes` first; it shows the symbol summary and raw diffs and records the exact reviewed change set. `Create / Update Pull Request` then creates or reuses the deterministic Workshop review branch, uploads those reviewed files, and creates or finds its open pull request. If local files change after review, submission stops until they are reviewed again.

OpenAI and GitHub secrets are encrypted with AES-GCM using a key held by Android Keystore. Preferences contain ciphertext rather than plaintext; installations upgrading from the earlier format migrate each legacy secret on first read and remove the plaintext only after encrypted storage succeeds.

GitHub uploads and pull-request operations share one serial background queue. The app persists whether an operation was queued, running, completed, failed, or interrupted. `Retry GitHub Operation` reconstructs failed work from current app-private files; pull-request retries also recheck the saved review fingerprint and stop if the files changed after review.

`Run AI Change` permits one active run. `Cancel AI` disconnects an active model request and prevents later model/tool turns. If cancellation arrives during an atomic source-write batch, that batch first reaches its existing compile/test/rollback boundary so source is never left partially applied. Any model call that already returned remains included in budget totals.

`Recent Commands` also shows bounded per-project AI outcomes, including cancellation/failure/rollback status, usage summaries when available, and the local trace path. `Retry Last AI` restores the latest recorded request and starts it again through the normal key, pricing, per-run, and monthly budget checks.

`Projects` is collapsed below the command workflow. The existing app-private `workshop_project` is adopted as `Bundled Workshop` without moving its files. `New Project From Sample` creates a separately identified, versioned app-private project; `Switch Project` changes the root used by symbols, compile, tests, AI history, and GitHub state and immediately recompiles it. Project creation/switching is blocked while AI or GitHub work is active or the source editor contains an unapplied edit. GitHub repository/branch targets and review/retry state are project-specific; the encrypted token is shared.

`Export Project Archive` opens Android's document-creation picker and writes a deterministic ZIP of the active project's normal files and versioned metadata. Generated `build/` output and temporary files are excluded. Export is limited to 512 files, 32 MiB per file, and 128 MiB total and does not require broad storage access.

Enter a new project name and choose `Import Project Archive` to restore an exported ZIP through Android's document picker. Import applies the same bounds, rejects duplicate/traversing paths and unsupported or incomplete metadata, preserves a freshly assigned local project identity, and deletes the new target if validation or extraction fails. A successful import becomes active and compiles immediately.

`Image Assets` stays inside the collapsed Projects panel. `Import PNG, JPEG, or WebP` reads only the document selected through Android's picker, rejects files over 8 MiB, 4096 pixels on either axis, or 16 megapixels, and stores a collision-safe copy under the active project's `assets/images/` directory. Tap a listed asset for preview, project-scoped selection, rename, or delete. Rename/delete stop when Stasis source references the asset path or filename. Confirmed deletion moves the file into a bounded project recovery queue, and `Restore Last Deleted Image` restores it without overwriting another asset. Images are included in project archive export/import and direct GitHub backup uploads their exact bytes alongside source.

`New Painted Image` opens the touch-first mini paint editor on a bounded 16-1024 pixel canvas. Existing library images up to 1024x1024 offer `Paint as Copy`. Brush/eraser, four sizes, palette and hex colors, bounded undo/redo, clear, and resize/crop all operate on an isolated in-memory bitmap. `Save as PNG` atomically creates a new project asset; Cancel leaves every accepted asset unchanged.

Selected library images appear in `Review AI Image Attachments` beside the command box with sampled thumbnails and remove controls. The next Run AI snapshots only those app-private project files, up to four images and 12 MiB total, and sends exact PNG/JPEG/WebP bytes as Responses API `input_image` data URLs at original detail. The attachment status shows an estimated GPT-5.6 Terra image-token/input cost before the call. Traces retain paths, dimensions, and estimates but never image Base64, picker URIs, or unrelated device media.

`Capture Preview for AI` reads the freshly rendered OpenGL framebuffer, bounds the retained capture to a 1024-pixel maximum axis, and pairs it with the exact logical frame commands used for that draw plus runtime/input context. The review dialog shows the pixels and separately opts rendered pixels and logical context into the next request; both default off. Remove clears the capture, switching projects discards it, pixels share the four-image/12 MiB budget, and neither pixel Base64 nor raw screenshot bytes enter AI traces.

`Allow one low-quality 1024x1024 AI image` is off by default and applies to one Run AI only. When enabled, the first agent turn can use the Responses image-generation tool with selected images as edit references. The current ~$0.006 low-quality output reserve is added to the normal Terra budget before the call and charged only if an image is returned. Generated PNG bytes remain temporary and absent from traces until the before/after review chooses `Accept as New Asset`; Reject and dialog dismissal leave the project unchanged, while acceptance creates a new collision-safe file and never overwrites its reference.

`Audio Assets` stays in the collapsed Projects panel. The Android document picker imports MP3, Ogg, WAV, or M4A only after the selected content is confirmed decodable, no larger than 16 MiB, and no longer than five minutes. `Record Audio` requests microphone permission on demand and captures bounded AAC/M4A; Stop & Save validates and atomically publishes it, while Cancel, app pause, or destruction deletes the temporary recording. Project switching, archives, AI, GitHub, and voice are blocked during capture. Tap an accepted item to preview, rename, or delete; Stop releases playback immediately. Referenced audio cannot be renamed or deleted until Stasis source is updated, and confirmed deletion uses a bounded project recovery queue. Accepted audio participates in archive import/export and bounded direct GitHub backup. Trim/normalize and AI audio attachment remain later AW57 work.

Unsaved manual source is autosaved on activity pause and instance-state save in a bounded per-project draft record. Recovery records the selected symbol identity and base-source SHA-256; startup restores the editor only if the same symbol still has the same base, so a stale draft cannot overwrite newer code. Apply, Reset, and baseline Revert clear only the matching draft. Rotation also restores the typed AI command, saved-symbol selection, menu/collapsed panels, editor scroll, and project-image selection paths. It deliberately drops screenshots/logical snapshots, one-run image generation, temporary paint/generated reviews, recording, voice, and playback so recreation cannot imply consent or accept temporary media. If the prior AI outcome remained `started`, startup marks it `interrupted` and warns that an in-flight paid request may have completed remotely; Retry deliberately starts a fresh budget-checked run rather than pretending an ambiguous HTTP call can resume safely.

`Privacy & Data` is collapsed with other infrequent settings and explains what stays local and which explicit actions send data. OpenAI and GitHub credentials can be revoked independently from encrypted storage without deleting project files or configuration. Pending image/screenshot/logical/generation consent can be cleared in one action. Confirmed AI activity erase removes command/outcome history for every project, usage and monthly-spend records, the local trace, and pending media. `Delete Active Non-Bundled Project` warns to export first, requires the exact name, switches to Bundled Workshop, and then removes that project's files/assets/trash, baseline, draft/recovery, and scoped AI/GitHub state; global credentials remain and Bundled Workshop cannot be deleted.

Accepted images and audio are registered in the shared `assets/manifest.json` contract used by the Rust runtime. Import, paint/AI save, recording, rename, delete, and restore update the manifest atomically; existing accepted assets are reconciled on the next asset mutation after an upgrade.

First run offers a deferrable zero-AI manual guide and requests no permission. `Start Manual Tutorial` opens the workshop menu and source editor on a real bundled symbol, then directs the user to edit, Apply, and Run Tests. The same checklist stays available under collapsed `Help & Onboarding`, together with Projects/archive backup and explanations that OpenAI, GitHub, media, and voice are optional; microphone permission is requested only when voice or audio recording is explicitly started.

Priority controls and media reviews include screen-reader descriptions, Workshop/section titles expose heading roles, status is a polite live region, and compile/test diagnostics are assertive. Menu, voice, source, AI Run/Cancel, symbols, asset rows, screenshots, generated comparisons, and paint all have explicit semantics. On screens narrower than 480 dp, AI progress and diagnostic/recovery actions stack vertically. Android lint passes; TalkBack, large-font, keyboard, orientation, tablet, and foldable acceptance still require device testing.

`Export Redacted Support Bundle` in Privacy & Data previews exactly what will be included, then uses Android's document picker for a bounded JSON export. It reports app/device versions, project category counts, coarse compile/reload and operation state, outcome status/usage presence, up to 50 recent trace event names, and any prior redacted crash type/class-method frames. The 64 KiB crash record excludes exception messages, filenames, line numbers, source, and paths, delegates to Android's normal crash handler, is reported next launch, and has a separate Clear control. The bundle builder never imports credentials, source, prompts, repository/project/file/media names or bytes, absolute paths, raw diagnostics, tool data, or raw trace fields.

Project metadata is format v2. Existing v1 projects migrate on first open only after their identity/origin is validated and a single fsynced v1 metadata backup is written; replacement is atomic and the migrated version/ID is reread for verification. V1 and v2 archives remain importable with a fresh local ID. Unknown future formats stop with an update instruction instead of being downgraded, and invalid v2 schema/origin stops without reseeding source, tests, or assets.

Sample and imported projects keep separate immutable source baselines. Sample baselines come from packaged assets; imported baselines come from the validated archive contents. Changes, Raw Diffs, Revert, and Reset therefore operate on the active project's own source, and imported projects are not silently filled with sample files. Direct GitHub backup uploads the complete active source set, while PR review remains limited to changes from that project's baseline.

Real-device touch acceptance uses the same packaged Rust/JIT runtime as the preview: an injected Android gesture updates Stasis `Input`, advances game logic, and moves the emitted player-paddle render command. The 2026-07-09 device run advanced 120 ticks during the check and moved the paddle command from Y 811 to Y 1537.

Failed manual Apply operations show the edited file/symbol, compiler result, and reload expectation. `Go to Diagnostic` reopens that symbol. `Recovery History` browses the bounded per-project journal, and `Undo Failed Apply` restores the selected entry only when the file still matches the failed version, so recovery cannot overwrite newer edits. Failed Stasis tests also populate diagnostic file/test/line navigation.
