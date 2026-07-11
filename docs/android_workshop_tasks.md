# Android Workshop PRD Tasks

The remaining Android Workshop PRD work is tracked in Maddox Tasks under parent
issue **#116** (`da0c5e05-fb79-46b8-a0ce-942934d3c8ff`). Use the released
`MaddoxTasks.exe agent issues` and `agent command` interfaces described by the
`maddox-tasks` skill; never edit the task database directly.

## Implementation

- #117 (`f2e9020e`) - Complete runtime sprite pipeline.
- #118 (`890eb692`) - Complete runtime audio mixer and backends.
- #119 (`d0519ded`) - Finish audio editing and AI attachments.
- #120 (`d98791f8`) - Add foreground and battery-aware background execution.
- #121 (`146448b1`) - Complete accessibility and adaptive layouts.
- #122 (`b9edd506`) - Complete data export, erase, and cache controls.
- #123 (`bd1cae96`) - Complete migration recovery UI and schemas.
- #124 (`cc5dd546`) - Detect and recover from restart loops.
- #125 (`8995958c`) - Finish the signed game-specific release matrix.
- #126 (`78624c35`) - Finish the Exploration Garden tutorial.
- #130 (`edf60a67`) - Finish GitHub sync semantics and acceptance.
- #134 (`af6a9a54`) - Reconcile the PRD checklist and current UI.

## Device acceptance

- #127 (`1b784243`) - Voice and shortcut layout.
- #128 (`ece911a2`) - Images, Paint, and multimodal AI.
- #129 (`f386b64a`) - Projects and lifecycle recovery.
- #131 (`45511a0f`) - Accessibility and orientation.
- #132 (`de3c8160`) - Queue connectivity and cancellation.
- #133 (`3661e209`) - Published runtime and performance.

## Execution loop

1. Select the highest-priority unfinished child of #116 that is safe for the
   current environment.
2. Move it to `Active` before implementation and record meaningful decisions as
   task comments.
3. Implement and validate it on its own branch/PR, or on the already active PR
   when it is a narrow continuation of that PR's scope.
4. Move completed work to `Ready for Review`, then select the next eligible
   child. Device-only tasks remain queued until the user confirms the device is
   ready for active testing.
