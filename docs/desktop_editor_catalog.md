# Compact graphical AI editor catalog

The graphical `stasis editor` sends its initial project catalog as indented text,
not an array of repeated JSON objects. Each import, global, constant, struct, and
function name occupies its own line beneath its file and category. Signatures, source bodies, and
canonical edit targets are loaded on request. Test descriptions are represented
by a per-file count until requested.

```text
src/main.stasis
  imports s0
    maze.stasis
  globals/constants s1
    phase
    level_index
  structs
    s2 Player
  functions
    s3 main
    s4 tick
tests/maze.test.stasis
  tests t7 (88; read to list names)
```

The labels are local to the immutable source snapshot for one editor request.
`read_source_symbol` accepts an `sN` label to retrieve the full original source
and canonical target. Edits use that returned target, never the short read ID.
A `tN` label returns the file's test names with their individual `sN` labels.
Different overloads or duplicate names have different IDs. Names and paths escape
embedded control characters so they cannot introduce extra catalog entries.
Independent reads can be batched; related edits retain atomic validation.

The shared request transcript remains JSONL and the OpenRouter HTTP envelope and
response schema remain JSON. The large project catalog inside that envelope is
plain text. This change applies to the graphical editor, not the live/TUI adapter.

## Measured acceptance

The real graphical-editor controller and OpenRouter serializer were captured
against a loopback server using RootbeerMaze3's saved task-2 and nightly-317 vendor
snapshot. No external AI call or project mutation occurred.

| UTF-8 bytes | Previous | Compact |
| --- | ---: | ---: |
| Project catalog | 73,200 | 14,551 |
| Initial model message | 75,813 | 18,071 |
| Complete HTTP body, including response schema | 86,442 | 22,449 |

The complete request passes a strict **25,000-byte** budget. These are byte counts,
not token estimates. All 300 source items remain available for on-demand reads;
88 test descriptions are deferred. Large existing task histories or attached
images can add payload beyond this initial text-only case.

Reproduce with the [desktop wire-capture test](reviews/desktop-editor-payload-20260912.md),
setting `STASIS_EDITOR_PAYLOAD_MAX_BYTES=25000` in addition to its project and output
variables. The test asserts the limit against the actual received HTTP bytes.

Visual evidence: not applicable; the inspected artifact is the captured text catalog.

Theory gained: omitting repeated identity metadata and deferring test descriptions
reduces this editor's request by about 74% without changing semantic edit identity.
