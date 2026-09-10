# Native editor layout validation

These images are deterministic native-renderer fixtures. They use synthetic task records and local repository assets; they do not show a live provider conversation or real user tasks.

Visual evidence: `fixture-wide-1100.png` proves the 1100-point wide hierarchy and persistent composer; `fixture-compact-520.png` proves active-task visibility and a non-overlapping compact composer; `fixture-highdpi-900-1.5x.png` proves crisp, unclipped fractional-scale rendering.

## Visual evidence

- `fixture-wide-1100.png`: 1100 x 900 point wide layout at 1.0 scale. The project rail, current-task state, provider/context metrics, timeline cards, and composer remain distinct and readable. The narrow partial card below the task header is the clipped beginning of older timeline content because the timeline intentionally stays pinned to its newest entries.
- `fixture-compact-520.png`: 520 x 900 point compact layout at 1.0 scale. The active `Add dash ability` selector is scrolled fully into view after task creation. The composer keeps both available actions visible without the `Ctrl+Enter sends` label colliding with `Run focused tests`; the reply field tooltip retains that shortcut.
- `fixture-highdpi-900-1.5x.png`: 900 x 900 point wide layout rendered to a 1350 x 1350 pixel image at 1.5 scale. Text, status outlines, task cards, and composer controls remain crisp and unclipped at fractional display scaling.

All three fixtures were opened and inspected at original resolution after capture.

## Recorded live plan review

The passed live report's two recorded semantic plans were rendered retrospectively through the production semantic diff renderer. This renderer reads the plans already stored in `../task-524/report.json`; it does not contact a provider, apply an action, or invent an execution result.

```powershell
$env:STASIS_EDITOR_REVIEW_REPORT = (Resolve-Path docs/evidence/ai-editor/task-524/report.json)
$env:STASIS_EDITOR_REVIEW_OUTPUT_DIR = (Resolve-Path docs/evidence/ai-editor/task-524)
python tools/cargo_cache.py run -- cargo test -p stasis --bin stasis desktop_editor::review_evidence::capture_recorded_live_proposal_reviews -- --nocapture
```

- `../task-524/recorded-review-01-task-1-main_stasis_render_accent_color_update.png`: the exact recorded RGB accent replacement, including the line-ending-only trailing-brace hunk, with task, provider, model, and action provenance.
- `../task-524/recorded-review-02-task-2-proposal-task-2-ball-readability.png`: the exact recorded white ball-halo insertion with enough surrounding draw-order context to assess it, with task, provider, model, and action provenance.

Both RGB PNGs were opened and inspected at original resolution after capture.

## Reference comparison

`../task-flow-reference.jpg` is a product-direction mockup rather than a captured application state. The native fixture preserves its main hierarchy: project/tasks at the edge, one task header, chronological work evidence, and a persistent bottom composer. The implementation uses the current Stasis lifecycle model and therefore shows provider/context usage, semantic-change states, and separate focused-test records.

Intentional differences from the reference include the native Windows frame capture, the `Tile Editor + Game` command, text-first task and activity cards, and the absence of the mockup's avatars, tags, decorative icons, generated-asset card, and separate game-window artwork. The fixture also pins a longer timeline to the bottom, so older content may begin with a partial card at the top of the viewport. Compact mode replaces the fixed project rail with a horizontally scrollable task selector and omits the inline keyboard hint when the action row is too narrow.

## Result

No overlap or clipping defect remains in the three captured configurations. Compact task navigation now reveals the active task without continually overriding user scrolling. Wide and high-DPI layouts preserve the same content structure, and the stronger borders and muted text remain legible against the dark panels.
