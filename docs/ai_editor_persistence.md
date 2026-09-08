# Desktop editor session recovery

The editor stores project-local history in `.stasis/editor/session.json`. This
directory is ignored by Git. The canonical project directory is part of the
versioned envelope: copying history into another project does not authorize that
project to load it.

The snapshot contains task chronology and semantic revisions, drafts, the active
task, provider display summaries, validation evidence, attachment references and
hashes, generated-image review state, and explicitly selected UI preferences.
Provider configuration, authorization headers, transport envelopes, and hidden
reasoning are outside the persistence model. User-authored messages and source
code are private content, not credentials storage; do not put secrets in them.

A save replaces the committed snapshot atomically. Recovery diagnoses malformed,
truncated, unsupported, or wrong-project state rather than interpreting it as an
empty successful restore. Schema migration supplies defaults for older optional
fields. Retention is bounded; overflow is reported rather than silently losing
semantic revisions or changing timeline positions.

The current schema is version 2; version 1 is migrated explicitly. Snapshots are
limited to 16 MiB, 32 tasks, and 1,024 distinct media sources, with additional
per-task limits inherited from the task model. Capacity errors leave the last
committed snapshot intact. Recovery errors prevent autosave from overwriting the
unreadable history; the user can inspect the state file or explicitly erase it.

Before admitting a provider request the editor saves an in-flight marker. A
restart treats such a marker as an uncertain outcome. Recovery never submits a
provider call. A user must explicitly send a new request, and the previous
call may already have incurred a charge.

On orderly shutdown, the editor waits for already executing host work, drains
its results through the normal task transitions, and saves again. This preserves
the applied action and receipt when an edit commits while the window is closing.

Validation fingerprints are compared to current project sources after loading.
Stale evidence cannot authorize completion. Semantic previews are recomputed
against the current compiler and source tree. Accepted and historical previews
are rebuilt only when their saved source fingerprint still matches; observed
staleness survives later restarts. Media references are rehashed;
missing, changed, or unverified files are unavailable for their previous review.
Interrupted focused tests and passing results without a source fingerprint are
reset to unverified. Window size is restored within safe desktop bounds; native
window handles, positions, and transient renderer state are not stored.

## Privacy and media lifecycle

Erasing saved history removes session state and resets the editor's in-memory
history. It does not erase project source files or referenced media. Attachments
can refer to existing user files, imported assets, or runtime capture artifacts;
a history reference is not proof of exclusive file ownership. Consequently no
startup recovery, retention operation, cancellation, or history erasure deletes
referenced media. Delete unwanted media explicitly using the owning artifact or
project file lifecycle. There is no automatic age-based media cleanup.

## Validation

Use the repository Cargo wrapper for storage and editor tests:

```powershell
python tools/cargo_cache.py run -- cargo test -p stasis_ai --lib session_store
python tools/cargo_cache.py run -- cargo test -p stasis --bin stasis desktop_editor
```

The recovery tests use local deterministic fixtures and never contact a paid
provider. They cover the persistence contract independently of live gameplay.

Acceptance coverage is split at the storage/editor boundary:

- `session_store::tests` checks round trips, v1 migration, interrupted atomic
  replacement, malformed/truncated state, project isolation, retention limits,
  missing/changed media, uncertain calls, and privacy erasure.
- `desktop_editor::persistence::tests` reopens editor instances to check drafts,
  task order, active task, revision payloads, provider summaries, expansion and
  window state, stale source validation, and fingerprint-bound preview recovery.
  It also checks explicit reconnection without replay, preservation of corrupt
  history, and receipts from host work that completes during shutdown.
- The opt-in native recovery fixture below renders the restored proposal and
  draft alongside the uncertain-call warning for visual inspection.

To capture the recovered editor on Windows, set `STASIS_EDITOR_EVIDENCE_RECOVERY=1`
and `STASIS_EDITOR_EVIDENCE_PNG` to an output PNG path, then run the editor test
command above with the filter `capture_native_task_timeline` and
`-- --test-threads=1`. This fixture saves and reopens history before rendering
the restored draft, semantic proposal, and uncertain-call warning.

The desktop attachment importer uses `.stasis/editor/media/` for persistent
sessions. Closing the editor retains those owned copies so saved hashes and
references remain valid. Explicit attachment removal can delete a copy created
in the current session; history erasure retains media. Temporary editor fixtures
continue to clean their temporary attachment storage on drop.
