# Graphical editor payload verification

This records the baseline capture before the [compact desktop catalog](../desktop_editor_catalog.md)
change. The same capture test now verifies the compact catalog and accepts an optional
`STASIS_EDITOR_PAYLOAD_MAX_BYTES` limit for the complete HTTP body.

The graphical `stasis editor` path uses `TaskController`, the desktop reply provider,
and the shared OpenRouter serializer. It does not use the live/TUI tool adapter.
Its initial model message is one JSONL header containing instructions, three tool
specifications, the response contract, task context, and the entire editable-symbol
catalog. Source bodies are fetched through `read_source_symbol` after that first turn.
The HTTP JSON body also contains the response JSON schema and provider routing.

A fresh native test build captured RootbeerMaze3's saved task-2 through that real
dispatch path on 2026-09-12. The model message was 75,813 UTF-8 bytes and the HTTP
body was 86,442 bytes. The 300-symbol catalog accounts for most of the message.
These are byte counts, not token counts. Neither JSON nor JSONL implies a compact
catalog: every entry currently repeats its metadata keys and selectors.

The historical `codex/eval-line-oriented-ai` branch contains file-grouped text
discovery (`compact_ai_symbols`) for the live/TUI adapter. That branch is not in
the main revision used for this verification, and does not implement the desktop
editor's source catalog. Its outer provider protocol remains JSONL as well.

## Reproduce without an external AI call

Run the ordinary fixture regression:

```powershell
python tools/cargo_cache.py run -- cargo test -p stasis --bin stasis desktop_editor_initial_http_payload_uses_the_real_dispatch_path -- --nocapture
```

To capture an existing project's active saved editor task, set these variables in
the child shell before the same command:

```powershell
$env:STASIS_EDITOR_PAYLOAD_PROJECT = 'D:/code/RootbeerMaze3'
$env:STASIS_EDITOR_PAYLOAD_OUTPUT = 'D:/code/output/rootbeermaze3-editor-wire-capture'
```

The test loads the saved session into memory, obtains the real source snapshot,
and runs the controller and desktop provider against a bounded loopback HTTP
server. Only the provider URL and credential are replaced. Routing, instructions,
catalog construction, response schema, and serialization use production code.
The server captures the raw body and returns a terminal response without tools.
The project and saved session are not written and no external AI call is made.
The output files are `http-body.exact.json` and `model-message.exact.jsonl`.

Validation: fixture and RootbeerMaze3 wire captures passed, along with five source
context tests and three request/image admission tests.

Visual evidence: not applicable; verification inspects the actual HTTP bytes.

Theory gained: payload conciseness depends on each editor adapter's catalog,
not just the shared transport format. A TUI discovery change does not reduce
the graphical editor's first request unless that adapter is updated too.
