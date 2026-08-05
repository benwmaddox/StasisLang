# Stasis Gauntlet: Autonomous Live Game Creation and Improvement

## Outcome

`stasis gauntlet` creates or improves a complete 2D Stasis game while one
authoritative graphical runtime remains active. A lead agent freezes a concrete
quality bar and divides the game into independently judged workstreams. Fresh
builders make one bounded project change at a time. Separate read-only critics
inspect real framebuffer captures, runtime evidence, and tests without seeing
the builder's rationale. Improved candidates become Git checkpoints;
regressions roll back automatically.

The loop continues until independent critics accept the result, the user stops
it, progress stalls, or the default eight-hour/100-model-call budget is
exhausted. A terminal event observer, JSONL stream, and generated progress page
expose the run without interrupting it.

The first release targets desktop JIT development and existing Stasis 2D
capabilities. Android Workshop can later reuse the orchestration contracts, but
its mobile UI and lifecycle remain separate.

## Method translated to Stasis

The useful mechanism in the [Gauntlet Loop
article](https://somethingbig.ai/gauntlet-loop) is the separation of goal,
artifact, builder, judge, and stopping authority. The published [Claude of Duty
prompt](https://github.com/mshumer/Claude-of-Duty/blob/main/prompt.md) leaves
decomposition to the lead, requires comparison with a real reference, and
repeatedly sends the largest shortcoming back for repair.

| Gauntlet principle | Stasis implementation |
| --- | --- |
| Give the destination, not an architecture | Store the immutable game brief; let the lead derive workstreams and implementation tasks. |
| Use a concrete bar | Freeze reference images, gameplay scenarios, state assertions, performance limits, and a scored rubric before production edits begin. |
| Split into independently improvable pieces | Track workstreams such as controls, game loop, enemies, UI, visual language, effects, audio, progression, and polish. |
| Builder must not grade itself | Builders may edit through controlled tools; critics receive artifacts and evidence but no write tools or builder reasoning. |
| Judge the real result | Capture the actual framebuffer, execute deterministic input scenarios, inspect runtime state, and run native tests. |
| Keep iterating | Continue without a fixed round count, bounded by convergence, cancellation, stagnation, or the configured resource budget. |
| Let the user watch without interrupting | Publish terminal events, JSONL events, and a static progress page with captures, scores, checkpoints, and current gaps. |
| Smooth independently improved pieces | Run a periodic integration workstream that can improve consistency but cannot redefine the quality bar. |

Stasis does not copy an unconstrained filesystem or self-reported-quality
workflow. Compiler-owned semantic edits, deterministic tests, state migration,
between-tick commits, and complete rollback remain authoritative.

## Product contract

### CLI

```text
stasis gauntlet new NAME --dir PATH --goal-file GOAL.md
    [--reference FILE]...
    [--discover-references]
    [--tui | --jsonl]
    [--max-hours 8]
    [--max-model-calls 100]

stasis gauntlet run
    [--workspace PATH]
    [--config gauntlet.json]
    [--reference FILE]...
    [--discover-references]
    [--tui | --jsonl]

stasis gauntlet resume RUN_ID [--tui | --jsonl]
stasis gauntlet status RUN_ID [--json]
stasis gauntlet stop RUN_ID
stasis gauntlet promote RUN_ID
```

`new` creates a Git-backed graphical seed and starts its first run. `run`
improves an existing project from `HEAD` in a linked isolated worktree.
Interactive terminals default to the human-readable terminal observer. `stop` cooperatively cancels
the active model or test, rolls back a provisional candidate, and retains the
best checkpoint. `promote` is the only operation that integrates an isolated
result into the user's original branch; it refuses a dirty or ambiguous
destination.

Convergence exits successfully. Budget exhaustion, stagnation, and an unmet
bar exit nonzero while preserving the best playable checkpoint and reporting
its branch and path.

### Configuration

The project root contains a strict, versioned `gauntlet.json`:

```json
{
  "schema_version": 1,
  "goal_file": "GAUNTLET_GOAL.md",
  "quality_bar": {
    "allow_web_discovery": true,
    "references": [],
    "required_scenarios": []
  },
  "budget": {
    "wall_time_minutes": 480,
    "model_calls": 100,
    "stalled_candidates": 5
  },
  "execution": {
    "autonomy": "full",
    "observer": "auto",
    "builder_max_turns": 30,
    "compaction": {
      "enabled": true,
      "max_request_bytes": 2097152,
      "retain_recent_turns": 6
    }
  },
  "models": {
    "scout": {},
    "lead": {},
    "builder": {},
    "visual_critic": {},
    "gameplay_critic": {}
  }
}
```

Unknown fields, unsupported versions, paths outside the project, zero budgets,
and invalid references fail before starting a model call. CLI budget flags
override one run without rewriting the project configuration. The goal and
frozen bar cannot be weakened after editing begins. Tests or stricter checks
may be added, but accepted requirements cannot be removed.

Ordinary `stasis ai` retains its 15-turn default. Gauntlet builders default to
30 turns and may be configured from 1 through 48, still bounded by the run's
total model-call and wall-time budgets. This gives a difficult workstream room
for multiple inspect/test/correct cycles without silently granting an
unlimited session.

When compaction is enabled, a builder request is compacted after it exceeds
the configured byte ceiling. Stasis retains up to the configured number of
recent raw turns and replaces older turns with deterministic summaries of
explicit working notes, tool names and targets, compile/test receipts, and errors.
Source bodies and other large observations are omitted from compacted history.
The immutable request header, current task, durable `decisions.jsonl` memory,
and recent turns remain available. Compaction consumes no model call and emits
an auditable `context_compacted` trace event with before/after byte counts.
Limits are 256 KiB through 16 MiB and 1 through 16 retained turns; the default
is 2 MiB and six turns.

Every role has an independent optional `model` and `reasoning_effort`. Empty
objects inherit `STASIS_AI_MODEL`, `STASIS_AI_REASONING_EFFORT`, and ultimately
the installed defaults. Values are passed directly to the installed Codex
CLI, so a model identifier must actually be supported there. For example, if
that installation exposes `luna`, a cost-oriented configuration can use:

```json
{
  "scout": {"model": "luna", "reasoning_effort": "low"},
  "lead": {"model": "gpt-5.6-sol", "reasoning_effort": "high"},
  "builder": {"model": "luna", "reasoning_effort": "medium"},
  "visual_critic": {"model": "gpt-5.6-sol", "reasoning_effort": "medium"},
  "gameplay_critic": {"model": "gpt-5.6-sol", "reasoning_effort": "high"}
}
```

This is policy rather than a hardcoded recommendation: cheaper models fit
reference scouting and bounded implementation work only when their observed
acceptance rate justifies it. The lead and independent critics can remain on a
stronger model. Model-call accounting stays global across roles.

Subscription-backed Codex is the first provider. Stasis does not request an API
key or estimate a dollar cost. The [Codex non-interactive command
surface](https://learn.chatgpt.com/docs/developer-commands#codex-exec) supplies
JSONL events, structured output, web search for the reference scout, and image
attachments for fresh critics.

ImageGen is an optional host capability for concept art, backgrounds, and
texture sheets. The current `codex exec` child transport accepts image inputs
but does not expose the in-product ImageGen tool, so the core CLI never assumes
it is present and never falls back to an API key. A host that can invoke
the built-in ImageGen tool copies the selected project-bound output into the
isolated worktree, then submits its PNG bytes plus provider/model/prompt
provenance to the same bounded PNG import transaction; it must not leave a
referenced asset only in the host's generated-images directory. Both Gauntlet
and the one-shot `stasis ai` command can use the import bridge; deterministic
raster composition remains their always-available fallback. In either case, a
critic judges the rendered in-game result, not the generator's preview.

The host bridge is `import_png_asset`: the host places a selected PNG under
`build/gauntlet/imagegen/`, then the same contiguous asset/source transaction
copies it under `assets/generated/` and derives the manifest entry. Imports
must be real PNG files, non-symlinks, at most 16 MiB, at most 2048 pixels per
edge, and at most 4,194,304 pixels total. This bridge is inert when the running
host has no ImageGen capability. One-shot `stasis ai` uses the equivalent
`build/ai-assets/imagegen/` inbox.

### Run records

Non-source artifacts live under:

```text
build/gauntlet/<run-id>/
  run.json
  events.jsonl
  decisions.jsonl
  usage.jsonl
  references/manifest.json
  artifacts/<candidate-id>/
  checkpoints.json
  index.html
  stop.request
```

`run.json` records the versioned phase, source commit, best checkpoint, current
workstream, budgets, exact model-call count, critic results, rollbacks, and
terminal outcome. Writes use atomic replacement. `events.jsonl` is append-only
and powers both the terminal observer and static report. The report shows reference
provenance, before/current captures, rubric scores, tests, usage, checkpoints,
rejected candidates, and the largest remaining gap.

`decisions.jsonl` is durable model working memory, not a dump of private
chain-of-thought. The `record_decision` tool stores bounded explicit
conclusions: kind, summary, concise rationale, evidence, and next step. The
controller also records lead choices, accepted/rejected checkpoints, and final
gate failures. Each append is flushed and synced so interruption or resume does
not erase the current theory of the game.

## Execution architecture

### Workspace isolation and seed

For a new game, Stasis creates a target project with a real `main`, `tick`,
`render`, and `on_code_swap`, a deterministic test, an empty v2 asset manifest,
and a visible blank-canvas scene. It commits that seed and creates
`stasis/gauntlet/<run-id>`.

For an existing game, Stasis resolves `HEAD` and creates that branch in a linked
worktree under the ignored `build/gauntlet/worktrees/` area. Dirty and
untracked user files are never copied or overwritten. Every compiler, test,
runtime, and agent action occurs inside the linked worktree. Each accepted
improvement becomes a narrow Git checkpoint. Tracked files in the original
checkout change only through explicit promotion.

### Reference and bar bootstrap

Before the first production edit, the harness validates and hashes local
references. When necessary, a fresh read-only scout runs with web search and
returns candidate source pages with provenance. Supplied local image
references are copied into the isolated run record, hashed, and attached to
the lead and critics. The first release deliberately does not download web
images: discovered pages establish the bar, while only user-supplied local
images become frozen visual evidence. References are never packaged or offered
as builder assets.

A fresh lead then freezes workstreams, rubric dimensions, required scenes,
input scenarios, state assertions, and completion thresholds. If the scout
cannot establish at least one usable visual reference and one measurable
gameplay bar, the run stops before production modification.

### Persistent runtime and live protocol

The controller owns one in-process graphical JIT runtime and one live-session
client. The observer consumes controller events rather than competing for live
responses.

The version-one live protocol gains two additive primitives:

- `set_input_state` injects a bounded logical pointer snapshot and temporarily
  overrides physical input during deterministic scenarios.
- `capture_frame` schedules a PNG for the next presented frame using a
  controller-generated artifact identity.

The controller composes existing `snapshot`, `step`, `inspect`, and `restore`
commands with those primitives into deterministic scenarios. Input is limited
to eight pointers with valid logical coordinates. Captures occur after drawing
and post-effects but before presentation, matching the existing screenshot
acceptance path. Ordinary rendering allocates no capture framebuffer.

### Agent roles and context separation

- The **reference scout** is read-only and web-enabled.
- The **lead** chooses the single highest-value next work item from the frozen
  bar, compact project state, and critic outcomes.
- A **builder** receives one work item, relevant captures, and the prior
  critic's largest gap. It changes the project through controlled tools only.
- A **visual critic** receives shuffled candidate images, references, and the
  frozen visual rubric. It receives no source or write tools.
- A **gameplay critic** receives deterministic scenarios, captures, state
  traces, and the gameplay rubric, but no production write tools.
- An **integration critic** periodically checks that the independently improved
  systems form one coherent game.
- A **smoother** is a normal restricted builder assigned only defects reported
  by the integration critic.

Every builder and critic starts with fresh context. Critics never receive
builder reasoning or learn which shuffled candidate is newer. Read-only critics
may run concurrently over immutable evidence; production builders remain
serialized because one live runtime and one transactional project state are
authoritative.

Fresh leads and builders receive a compact chronological projection of the
latest 48 decision records, capped at 32 KiB. Builders may call
`record_decision` during exploration and after tested choices, so architectural
decisions and failed approaches survive context boundaries and `resume`.
Controller outcomes use the same journal, and the latest recorded next step
restores the working gap after a restart. Blind visual and gameplay critics are
never given this memory; they receive only anonymous evidence, the frozen bar,
and reference material.

### Project and asset transaction

A versioned project patch combines compiler-owned source/test edits with SVG,
deterministically composed PNG, validated host-generated PNG imports, bounded
JSON/CSV data, deterministic procedural WAV generation, and compiler-derived
asset-manifest entries. The model never writes the asset manifest directly.
Deterministic PNG creation uses
a bounded background plus rectangle/circle/line scene description (maximum
2048 per dimension, 4,194,304 pixels, and 512 shapes), so the model does not
need to emit opaque base64 blobs.

The harness stages the complete patch, validates asset preparation, compiles,
runs all project tests, computes the migration preview, and then publishes
files, prepared assets, state migration, code pointers, and renderer reload at
a between-tick boundary. Any failure restores source, assets, manifest, code,
and state.

Full autonomy may apply a validated layout migration because the run is
isolated and checkpointed. Existing `stasis ai` and TUI approval behavior does
not change. Asset updates are synchronized into the prepared play bundle before
the frame commit. New assets must be loaded by accepted code or
`on_code_swap`.

The initial scope supports vector art, generated PNG sprites, renderer
primitives, structured data, and procedural audio. Network references cannot
become game assets. Photo synthesis and acquisition of third-party raster
assets are not part of version one.

### Candidate loop

For every selected work item:

1. Capture the best accepted baseline using the relevant deterministic
   scenarios.
2. Run one fresh builder and apply its tested patch provisionally.
3. Capture the candidate with identical inputs and initial state.
4. Run compile/tests, scenario assertions, renderer diagnostics, missing-asset
   checks, performance budgets, and state/layout invariants.
5. Run fresh visual and gameplay critics required by the workstream.
6. Shuffle baseline/candidate labels for direct A/B comparison.
7. Accept only when all hard gates pass, no required dimension materially
   regresses, and the critics prefer the candidate or confirm that it closes a
   frozen blocker.
8. Commit an accepted candidate as the next baseline.
9. Roll back a regression completely and count it toward stagnation.
10. Feed only the largest evidenced gap into the next lead decision.

Convergence requires all hard gates plus two separate final evaluations that
mark every required rubric dimension as meeting the frozen bar. A merely
improved result is not labeled converged. Five consecutive non-improving
candidates stop as `stalled`; eight hours or 100 model calls stop as
`budget_exhausted`. Both retain the best checkpoint.

Cancellation is observed between model calls, tool batches, tests, scenario
steps, and commit boundaries.

## Delivery slices

1. Add strict configuration/run/event/critique/scenario/patch contracts, the
   CLI family, this document, and the real graphical seed.
2. Generalize `stasis_ai` for explicit roles, schemas, images, search, model
   settings, and exact call accounting while preserving live AI behavior.
3. Add deterministic input injection and dynamic framebuffer capture, then
   compose those with existing snapshot/step/inspect/restore commands.
4. Add controlled SVG/data/WAV operations and atomic asset-aware project
   patches.
5. Implement reference bootstrap, immutable bars, builders, blind critics,
   scoring, rollback, Git checkpoints, budgets, and finalization.
6. Add the event-driven observer, static progress page, stop/resume, and
   explicit promotion.
7. Update CLI/live/asset/contributor guidance and remove duplicate or obsolete
   paths introduced during the slices.

## Test and acceptance plan

Routine orchestration tests use deterministic fake providers and never consume
subscription calls. Cover:

- strict configuration and path validation;
- budgets, cancellation, stagnation, convergence, and recovery;
- reference path validation, hashing, copying, and page provenance;
- critic label shuffling and source/write isolation;
- exact model-call and token-usage accounting without dollar estimates;
- input injection, scenario limits, capture timing, and state restoration;
- PNG dimensions, hashes, and renderer failure diagnostics;
- mixed source/test/asset rollback at every failure point;
- compatible and incompatible migrations under Gauntlet autonomy;
- existing live AI, TUI, and stdio compatibility;
- dirty original workspaces remaining byte-for-byte unchanged;
- stop/resume from every persisted phase; and
- promotion refusal on dirty or non-fast-forward destinations.

Add a compact playable end-to-end fixture whose baseline intentionally misses
a visible and behavioral bar. It must compile through the real frontend and
Cranelift JIT, launch the graphical runtime, execute deterministic input,
capture a baseline, apply a source-plus-SVG candidate, preserve state, capture
the improvement, reject and roll back a regression, and package/run the best
checkpoint as a desktop executable.

Provide an opt-in live-provider acceptance command that creates a small complete
game, discovers references, accepts at least two workstreams, rejects one
candidate, and leaves a playable checkpoint plus report. Each individual model
or test command remains below 15 minutes; the eight-hour limit applies only to
an explicitly started product run.

Completion requires:

- `gauntlet new` launches a graphical seed before its first improvement;
- source, tests, and controlled assets commit atomically;
- compatible swaps preserve the current match;
- critics inspect real captures and cannot edit production files;
- regressions restore source, assets, state, and runtime;
- the frozen bar cannot be relaxed;
- web references have provenance and are never packaged;
- stop, budget, stagnation, and recovery retain the best checkpoint;
- the observer never blocks the game tick;
- the original checkout remains untouched until promotion; and
- the final checkpoint passes format, compile, tests, scenarios, desktop build,
  package, and executable smoke checks.

## Boundaries and defaults

- Version one creates and improves complete 2D desktop games, not 3D or AAA
  engines.
- Full autonomy applies only inside the isolated run branch/worktree.
- Defaults are eight hours, 100 model calls, and five consecutive
  non-improving candidates.
- Web discovery is enabled when local references are insufficient; automatic
  downloading of discovered images is deferred.
- Mutation is serialized; parallel builder merges are deferred.
- References guide comparison only and are not shipped.
- Arbitrary photo synthesis inside the CLI, copyrighted asset acquisition,
  video understanding, Android execution, and unattended promotion are out of
  scope. Deterministic PNG composition is included; a capable host may supply
  ImageGen output through the same validated PNG transaction.
