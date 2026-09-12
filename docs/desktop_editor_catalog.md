# Compact AI source catalog

The graphical `stasis editor` and live TUI use one byte-bounded project index and
selector implementation. The index is indented text.
Every project file has a short ID and counts for imports, globals, structs,
functions, and tests. Function and struct names use one line each. Import and
global names, source bodies, signatures, and canonical edit targets load on request.

```text
files
  f0 src/main.stasis i3@0 g2@1 s1 f2 t0
  f1 tests/maze.test.stasis i2@5 g0 s0 f0 t88
vendor
  stasis/stdlib/graphics.stasis
symbols
  f0 structs
    Player
  f0 functions
    main
    tick
```

An `@N` selector reads the import or global group advertised beside its count.
`fN:name` reads a displayed function or struct. A category may declare a shared
prefix once; the host restores it when resolving the nested name. `?terms` searches
all symbol and global names, while `?+terms` includes source for the best matches.
Independent calls can be batched. Returned canonical targets retain exact identity
for duplicate names and edits.
Declarations marked `@internal`, along with declarations from internal source files,
are excluded from both the initial catalog and its on-demand source snapshot.

Both AI surfaces use the compact line-oriented request frame. The OpenRouter HTTP
envelope and response schema remain JSON. Their edit execution stays surface-specific:
the graphical editor returns one proposal for approval, while the TUI applies one
contiguous atomic write batch through the live runtime. Both resolve reads through
the same catalog selectors and canonical source targets.

## Measured acceptance

The real graphical-editor controller and OpenRouter serializer were captured
against a loopback server using RootbeerMaze3's saved task-2 and nightly-317 vendor
snapshot. No external AI call or project mutation occurred.

| UTF-8 bytes | Previous | Compact |
| --- | ---: | ---: |
| Project catalog | 14,551 | 4,684 |
| Initial model message | 18,433 | 6,056 |
| Complete HTTP body, including response schema | 22,811 | 8,746 |

The complete request passes a strict **10,000-byte** budget and the catalog passes
a **5,000-byte** budget. These are byte counts,
not token estimates. All 300 source items remain available for on-demand reads;
88 test descriptions are deferred. Large existing task histories or attached
images can add payload beyond this initial text-only case.

Reproduce with the [desktop wire-capture test](reviews/desktop-editor-payload-20260912.md),
setting `STASIS_EDITOR_PAYLOAD_MAX_BYTES=10000` in addition to its project and output
variables. The test asserts the limit against the actual received HTTP bytes.

Visual evidence: not applicable; the inspected artifact is the captured text catalog.

Theory gained: a complete file/count manifest plus prefix-compressed function names
keeps the project discoverable while host-side search defers its largest name groups.
