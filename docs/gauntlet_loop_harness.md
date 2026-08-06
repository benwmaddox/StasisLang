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
improves a clean existing project directly on its current branch by default.
Set `execution.isolation` to `worktree` when a separate linked checkout is
explicitly desired.

The graphical seed includes `assets/gauntlet-ui.ttf`, its SIL Open Font License,
and a manifest declaration. The seed loads and renders that project-local font,
so builders can create readable HUD text without relying on machine-specific
system font paths.

`budget.model_calls` is an admission budget for starting new candidate cycles.
After a candidate starts, a configured builder escalation receives a fresh
`execution.builder_max_turns` allowance and the controller completes both
independent critiques, even if doing so crosses the admission budget. The total
counter still records every call, and the controller will not admit another
candidate once the configured budget is exhausted. This prevents a primary
builder from leaving only a token one-turn escalation or an unevaluated edit.
`budget.wall_time_minutes` bounds one active controller session. `resume`
starts a fresh wall-time session while preserving the original run start,
model-call count, accepted checkpoint, critiques, and decision memory, so host
downtime does not consume the working budget.

Interactive terminals default to the human-readable terminal observer. `stop` cooperatively cancels
the active model or test, rolls back a provisional candidate, and retains the
best checkpoint. For the default in-place mode, accepted checkpoints are
already on the project branch and `promote` is an idempotent confirmation. For
explicit worktree isolation, `promote` fast-forwards the original clean branch
and refuses a dirty or ambiguous destination.

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
    "isolation": "in_place",
    "builder_max_turns": 30,
    "compaction": {
      "enabled": true,
      "max_request_bytes": 2097152,
      "retain_recent_turns": 6
    }
  },
  "models": {
    "scout": {"model": "gpt-5.6-luna", "reasoning_effort": "max", "timeout_minutes": 30},
    "lead": {"timeout_minutes": 30},
    "builder": {"model": "gpt-5.6-luna", "reasoning_effort": "max", "timeout_minutes": 30},
    "builder_escalation": {"model": "gpt-5.6-sol", "reasoning_effort": "high", "timeout_minutes": 30},
    "controller_escalation": {"model": "gpt-5.6-sol", "reasoning_effort": "high", "timeout_minutes": 30},
    "visual_critic": {"timeout_minutes": 30},
    "gameplay_critic": {"timeout_minutes": 30}
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

A builder must not spend that allowance repeating a terminal failure. The
Gauntlet-only `report_blocked` tool records the reason, evidence, and required
next step, then terminates the attempt in the same turn. The controller can
immediately apply the configured one-shot builder escalation. If the rescue
builder reports the same non-recoverable condition, the candidate is rejected
without consuming the rest of its turn allowance.

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

Every role has an independent optional `model` and `reasoning_effort`, plus a
`timeout_minutes` value that defaults to 30 and may be configured from 1
through 120. New
Gauntlet configurations default the scout and builder to `gpt-5.6-luna` with `max`
reasoning. If the primary builder cannot finish, the same candidate receives one
bounded rescue attempt from `gpt-5.6-sol` with `high` reasoning. Lead and both critics inherit `STASIS_AI_MODEL`,
`STASIS_AI_REASONING_EFFORT`, and ultimately the installed defaults. An explicit
empty role object also selects that inherited behavior. Values are passed
directly to the installed Codex CLI, so a model identifier must actually be
supported there. A fully explicit configuration can use:

```json
{
  "scout": {"model": "gpt-5.6-luna", "reasoning_effort": "max", "timeout_minutes": 30},
  "lead": {"model": "gpt-5.6-sol", "reasoning_effort": "high", "timeout_minutes": 30},
  "builder": {"model": "gpt-5.6-luna", "reasoning_effort": "max", "timeout_minutes": 30},
  "builder_escalation": {"model": "gpt-5.6-sol", "reasoning_effort": "high", "timeout_minutes": 30},
  "controller_escalation": {"model": "gpt-5.6-sol", "reasoning_effort": "high", "timeout_minutes": 30},
  "visual_critic": {"model": "gpt-5.6-sol", "reasoning_effort": "medium", "timeout_minutes": 30},
  "gameplay_critic": {"model": "gpt-5.6-sol", "reasoning_effort": "high", "timeout_minutes": 30}
}
```

This default puts the discounted model on high-volume bounded work while the
lead and independent critics remain on the stronger installed default. Projects
can override any role when observed acceptance quality warrants it. Set
`models.builder_escalation` to another role object to
change the rescue tier, use `{}` to inherit the installed defaults, or use
`null` to disable builder escalation. Failed primary calls and rescue calls both
count against the global model-call budget. `models.controller_escalation`
provides the same one-shot recovery for scout, lead, and critic calls. If both
critic attempts fail, the provisional candidate is rolled back and counted as
a bounded stall; it is never accepted without independent evidence.
The controller reserves two calls before starting or escalating a builder, one
for each independent critic. It will end as `budget_exhausted` instead of
spending the final calls on a candidate that can never be evaluated.

Subscription-backed Codex is the first provider. Stasis does not request an API
key or estimate a dollar cost. The [Codex non-interactive command
surface](https://learn.chatgpt.com/docs/developer-commands#codex-exec) supplies
JSONL events, structured output, web search for the reference scout, and image
attachments for fresh critics.

ImageGen is a host capability for authored game art. Gauntlet requires at least
one fulfilled, transactionally imported, loaded, and visibly drawn ImageGen PNG whenever the selected
workstream is explicitly art, visual, graphics, sprite, illustration, or
animation work. The current `codex exec` child transport accepts image inputs
but does not expose the in-product ImageGen tool directly. An agent instead
calls `request_imagegen_asset`; Stasis persists the exact prompt and intended
use under the project-bound ImageGen inbox and waits up to 30 minutes for a
capable host to save the requested PNG. The observation returns the validated
`source_path`, which the same agent passes to `import_png_asset` in its later
atomic asset/source write batch. The core CLI never assumes a provider or falls
back to an API key. A host must not leave a referenced asset only in its own
generated-images directory. Both Gauntlet and the one-shot `stasis ai` command
use this handshake; deterministic raster composition remains their
always-available fallback. In either case, a critic judges the rendered in-game
result, not the generator's preview.

Request one isolated asset or subject per generated image, not a multi-item
atlas. Masters default to 1024x1024; an agent may request up to 2048x2048 when
extra detail or crop latitude is justified. Leave that master in the ignored
ImageGen inbox. The transactional import may
copy it unchanged, crop it, or remove a flat background to create the tracked
game asset without degrading the reusable source.

The requested width and height are generation targets rather than a reason to
stall on an otherwise valid provider result. The bridge accepts any decodable
PNG within the bounded import limits and returns both its actual and requested
dimensions plus `dimension_adjusted`. The builder can then crop or scale its
runtime presentation deliberately instead of waiting 30 minutes for an exact
provider size that some host generators do not guarantee. Invalid, oversized,
or non-PNG results remain rejected.

When a replacement makes an older generated asset obsolete, the builder uses
`delete_asset` in the same contiguous asset/source transaction. The tool removes
the controlled file, its matching manifest entry, and any prepared-cache copy;
all three are restored if the related source edit fails. Deletion precedes a
replacement that reuses the same asset id.

For those authored-art workstreams, Gauntlet hides primitive SVG and
shape-composed PNG tools and rejects completion until an ImageGen request has
been fulfilled and its PNG imported through the atomic asset/source transaction,
referenced by project Stasis source, loaded, and emitted through a sprite draw
path. Merely adding an unused PNG to the manifest does not satisfy the gate.
Primitive rendering remains appropriate for basic UI, simple icons,
selection/range overlays, and deterministic fallbacks. ImageGen remains optional
for work that is purely logic or basic interface geometry.

The current render-v2 category order is clear, line primitives, sprites, then
text. An opaque full-board sprite therefore covers a battlefield that is still
drawn with lines. Until a project has a verified sprite-first background path,
agents request isolated foreground subjects on flat removable backgrounds and
use bounded crop/background removal during import. This keeps generated units,
buildings, and props above the board while preserving grid and tactical overlays.

The request is stored under `build/gauntlet/imagegen/requests/`. The host places
the selected PNG at the request's `output_path` under
`build/gauntlet/imagegen/`, then the same contiguous asset/source transaction
copies it under `assets/generated/` and derives the manifest entry. Imports
must be real PNG files, non-symlinks, at most 16 MiB, at most 2048 pixels per
edge, and at most 4,194,304 pixels total. `import_png_asset` can copy the image
unchanged, crop a bounded rectangle, and remove a caller-selected flat background
color with a bounded tolerance before saving the project PNG. This bridge is inert when the running
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
`report_blocked` tool writes the same durable evidence before terminating an
impossible builder attempt. The
controller also records lead choices, accepted/rejected checkpoints, and final
gate failures. Each append is flushed and synced so interruption or resume does
not erase the current theory of the game. A failed builder attempt extracts a
bounded, deduplicated summary of tool errors from its trace, including repeat
counts, and records that evidence before any rescue starts. The rescue receives
the refreshed decision memory in its prompt rather than the primary builder's
stale initial snapshot. Atomic-write and completion-gate failures are also
flushed into decision memory when the live tool returns them, before the agent
can retry. A controller or machine interruption during that attempt therefore
does not strand the only useful diagnostic in a raw trace.

At start and resume, the controller imports up to 12 recent failure and
rejection lessons from the four newest prior runs. Imported records retain the
source run, source event kind, and source timestamp and are idempotent on
resume. Legacy failure events are enriched from traces only when the resolved
trace is inside the project's `build/ai-traces/` directory. Raw traces remain
separate; only bounded explicit errors enter model-visible memory.

## Execution architecture

### Workspace and seed

For a new game, Stasis creates a target project with a real `main`, `tick`,
`render`, and `on_code_swap`, a deterministic test, an empty v2 asset manifest,
and a visible blank-canvas scene. It commits that seed and runs on that new
repository's current branch. The seed imports only the command-buffer and
window-request runtime modules that it materializes. Game code requests its
initial window through the runtime mailbox helper; payload fields are written
before the sequence counter publishes the request. This stays on the same
batched guest-to-host boundary as rendering instead of adding a startup-only
extern call or exposing ABI globals in generated game code.

For an existing game, Stasis requires a clean checkout and operates directly
on the current branch. Every accepted improvement becomes a narrow Git
checkpoint, so the main project always shows the latest accepted source and
assets. Rejected provisional edits are restored to that checkpoint. When
`execution.isolation` is explicitly `worktree`, Stasis instead creates a
`stasis/gauntlet/<run-id>` branch under the ignored
`build/gauntlet/worktrees/` area and leaves the original checkout unchanged
until promotion.

### Reference and bar bootstrap

Before the first production edit, the harness validates and hashes local
references. When necessary, a fresh read-only scout runs with web search and
returns candidate source pages with provenance. Supplied local image
references are copied into the isolated run record, hashed, and attached to
the lead and critics. The first release deliberately does not download web
images: discovered pages establish the bar, while only user-supplied local
images become frozen visual evidence. References are never packaged or offered
as builder assets.

A project may provide an authoritative `CREATIVE_DIRECTION.md` beside
`stasis.json`. The controller reads it with the same bounded-text protections as
the goal, freezes its hash and verbatim contents, and rejects cross-run reuse if
the source changes. A creative-director bootstrap turns that source plus the
immutable goal into a structured, controller-owned operational digest. The run
stores the source and digest in `quality-bar.json` and combines them in a
human-readable `creative-direction.md`; together they cover the
narrative promise, player fantasy, rule pillars, visual language, interaction
grammar, progression/pacing, and non-negotiables. It is authoritative for the
run, survives resume and fresh-agent boundaries, and is supplied to every lead,
builder, and critic. A later run with the identical goal hash reuses the newest
version-two direction and workstream decomposition, so a budget boundary does
not cause creative drift or spend another director call. Builders may implement
or refine it but cannot silently rewrite the game's identity to make a local
task easier.

The director also freezes workstreams, rubric dimensions, required scenes,
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
- The **lead** operationalizes the frozen creative direction as the
  playability and visual-coherence director. It inspects paired initial and
  post-probe frames,
  compact runtime/test evidence, references, and critic outcomes before choosing
  the single highest-value next work item. Its required playability guidance
  identifies which board relationships are unclear and tells the builder how a
  new player should recognize cells, terrain, factions, unit roles, selection,
  movement, combat previews, objectives, economy, turn ownership, end turn, and
  cancel/reselect without inventing unsupported mechanics.
- A **builder** receives one work item, relevant captures, and the prior
  critic's largest gap. It also receives the lead's playability guidance as a
  distinct instruction so visual polish cannot silently obscure the board's
  interaction grammar. It changes the project through controlled tools only.
- A **visual critic** receives shuffled initial/post-input image pairs,
  references, the frozen direction, and the visual rubric. In addition to the
  relative A/B verdict, it must separately record whether pixels affirmatively
  communicate current state, available actions, board semantics, and action
  feedback, with observed evidence for each anonymous candidate. It receives no
  source or write tools.
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

Every ordinary `stasis ai` agent and Gauntlet builder also receives two completed
discovery payloads before its first model turn. `initial_symbols` contains compact
signatures for the entry file and its direct imports. `stdlib_api` contains the
bounded public API catalog for the project-matched Stasis standard library,
including canonical import paths and function, struct, and constant signatures.
The catalog includes top-level public modules such as graphics, audio, collision,
layout, timing, storage, memory, and HUD helpers; it excludes internal host ABI,
test-only modules, globals, and function bodies. Agents should use this catalog
directly rather than spending turns rediscovering standard-library implementation
files.

Fresh leads and builders receive a compact chronological projection of the
latest 48 decision records, capped at 32 KiB. Builders may call
`record_decision` during exploration and after tested choices, so architectural
decisions and failed approaches survive context boundaries and `resume`.
Controller outcomes use the same journal, and the latest recorded next step
restores the working gap after a restart. A rescue builder sees the primary
attempt's exact bounded failure evidence, and later candidates see recent
provenance-tagged lessons imported from earlier runs. Blind visual and gameplay
critics are never given this memory; they receive only anonymous evidence, the
frozen bar, and reference material.

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

Full autonomy may apply a validated layout migration because the run starts
clean and every accepted state is checkpointed. Existing `stasis ai` and TUI approval behavior does
not change. Asset updates are synchronized into the prepared play bundle before
the frame commit. New assets must be loaded by accepted code or
`on_code_swap`.

The initial scope supports vector art, generated PNG sprites, renderer
primitives, structured data, and procedural audio. Network references cannot
become game assets. Photo synthesis and acquisition of third-party raster
assets are not part of version one.

### Candidate loop

For every selected work item:

1. Capture initial and post-input frames for the best accepted baseline using
   the relevant deterministic scenarios.
2. Run one fresh builder and apply its tested patch provisionally.
3. Capture matching initial and post-input candidate frames with identical
   inputs and initial state.
4. Run compile/tests, scenario assertions, renderer diagnostics, missing-asset
   checks, performance budgets, and state/layout invariants.
5. Run fresh visual and gameplay critics required by the workstream.
6. Shuffle baseline/candidate labels for direct A/B comparison.
7. Accept an incremental checkpoint when all hard gates pass, neither critic
   reports a regression, and at least one critic prefers the candidate. A
   visual-first or gameplay-first slice may pair one preference with an
   `equivalent` verdict from the unchanged dimension. Absolute scores still
   control final convergence.
8. Commit an accepted candidate as the next baseline.
9. Roll back a regression completely and count it toward stagnation.
10. Feed only the largest evidenced gap into the next lead decision.

Convergence requires all hard gates plus two separate final evaluations that
mark every required rubric dimension as meeting the frozen bar. Each final
evaluation must also affirm from the captured pixels that current state,
available actions, board semantics, and the result of the fixed input probe are
clear. These absolute comprehension gates do not prevent a genuinely improved
candidate from becoming the next incremental checkpoint. A merely improved
result is not labeled converged. Five consecutive non-improving
candidates stop as `stalled`; eight hours or 100 model calls stop as
`budget_exhausted`. Both retain the best checkpoint.

Cancellation is observed between model calls, tool batches, tests, scenario
steps, and commit boundaries.

Each running controller writes a two-second heartbeat. `gauntlet status`
reports `active`, `terminal`, or `interrupted; resume is safe`; `resume` refuses
to start while a fresh heartbeat exists, preventing two controllers from
mutating the same project workspace. A stale or missing heartbeat on a
non-terminal phase is recoverable: resume restores the latest accepted Git
checkpoint, retains the quality-acceptance streak and decision journal, clears
the old stop request, opens a fresh wall-time session, and restarts the loop.
Stopped, stalled, budget-exhausted, and failed runs are also explicitly
resumable after their cause has been addressed; only a converged run is final.
Model timeouts, invalid structured
responses, and transient role failures consume their real call budget and
produce explicit attempt events. Scout/lead bootstrap has a bounded
deterministic fallback; candidate capture or exhausted critic recovery rolls
back safely and advances the stall counter.

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
candidate, and leaves a playable checkpoint plus report. Each model call
defaults to a 30-minute deadline. Each individual test command remains below
15 minutes; the run-wide limit applies only to an explicitly started product
run and may be configured for day-long operation.

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
- in-place mode leaves the project at its latest accepted checkpoint, while
  explicit worktree mode leaves the original checkout untouched until
  promotion; and
- the final checkpoint passes format, compile, tests, scenarios, desktop build,
  package, and executable smoke checks.

## Boundaries and defaults

- Version one creates and improves complete 2D desktop games, not 3D or AAA
  engines.
- Full autonomy requires a clean Git checkout and applies only inside the
  selected project workspace; in-place is the default and worktree isolation
  is opt-in.
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
