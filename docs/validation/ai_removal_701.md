# Maddox #701: remove AI editing

## Scope and consumer audit

Removed desktop graphical AI editor, TUI AI interaction, Gauntlet AI, shared stasis_ai provider/session/task code, Android provider/login/chat/tool/verification/recovery/image-generation UI and native Codex bridge, AI-only tests/docs/evidence and packaging dependencies. The remaining live CLI requires a script or stdio and retains project operations, diagnostics and compiler editing tools.

Retained compiler semantic-edit APIs, runtime/rendering, LSP, project navigation, build/play, asset import/painting/audio, GitHub project sync and manual Android test diagnostics. WorkshopProjectSnapshot replaces the AI-named snapshot helper because manual runtime acceptance still consumes snapshot/restore/fingerprint. The Gauntlet font remains consumed by runtime text raster tests. Android JNI packaging excludes stale locally generated Codex libraries.

## Validation

- Cargo check for stasis all targets passed.
- Stasis CLI focused suite: 120 passed; library suite: 334 passed.
- Android JVM tests: 138 passed; direct Java compile passed.
- Fresh release native bridges built for arm64-v8a and x86_64; Workshop debug APK assembled and installed.
- Android shell contract, unsafe-boundary/nightly tests (11), AI-removal guards (2) passed.
- Android emulator lifecycle acceptance passed using a new Exploration Garden project, including real compiled preview frames. Receipt: artifacts/android_device_acceptance/workshop_20260923T214602Z.json (local artifact).
- Repository gate progressed through unit suites but failed desktop_hot_swap_generation_seam: local cargo CLI has no verified build fingerprint and refuses installed runtime startup. Exact focused rerun reproduced this startup prerequisite failure before rendering. Full gate is not claimed passing.
- Initial emulator acceptance selected an old persisted Render Parity project with a removed begin_frame call. Creating an additive current template project resolved that validation setup issue without clearing app data.

Visual evidence: inspected ../evidence/task701-ai-removal/onboarding.png, projects.png and preview.png. Onboarding and project controls expose retained manual workflows; preview shows Exploration Garden rendering with live tick/render timing. Screenshots do not establish desktop playback behavior.

## Follow-up boundaries

Maddox #715 separately gathers requirements for a future Android code/assets quick-test workflow. This change does not implement it. AI-specific follow-ups #424, #516, #566, #581, #628-632, #690 and #691 should be reviewed for obsolescence by the scheduler.

Theory gained: project snapshot transactions and compiler semantic edits are shared manual infrastructure, while provider/chat orchestration is removable at its consumers. Passing manual acceptance tests and a real Android preview support that boundary; future code/assets loading should connect to project activation and preview rather than recreate provider state.

Good: consumer audits preserved shared snapshots, semantic editing and runtime font assets.
Bad: persisted emulator content and local desktop build provenance complicated broad validation.
Adjustment: use a current additive sample project and report exact startup prerequisites separately from focused suite results.
