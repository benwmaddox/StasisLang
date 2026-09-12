# Compact AI source catalog

The graphical `stasis editor` and live TUI use one byte-bounded project index and
selector implementation. The index is indented text. Stable guidance comes first,
followed by the file map, the entry file's symbols, and two real entry-point
examples when `main` or `on_code_swap` is small enough. Other files expand on
request.

```text
files [id path]
  f0 src/main.stasis
  f1 tests/maze.test.stasis
vendor
  stasis/stdlib/graphics.stasis
entry f0 src/main.stasis
  imports s0
  globals s1
    player
  structs
    s2 Player
  functions
    s3 main
    s4 tick
    s5 on_code_swap
examples
  s3
    function main(): i32 {
      return 0;
    }
  s5
    function on_code_swap(): void {
      return;
    }
```

Catalog IDs are hypermedia links, not semantic identity. Each is one letter plus
digits: `f1` lists that file's symbols and `s4` returns exact source. Models copy a
displayed ID rather than substituting a path or metavariable. `search terms`
searches all symbol and global names, while `search-source terms` includes source
for the best matches. Older selector forms remain accepted for compatibility but
are omitted from the initial guidance.

Each result supplies current source and a canonical target. That result is a
stepping stone to additional linked lookups or a file change; it is not expected to
contain everything needed to finish every task. Independent visible links should
be followed together in one batched response. Returned canonical targets retain
exact identity for duplicate names and edits. Declarations marked `@internal`,
along with declarations from internal source files, are excluded from both the
initial catalog and its on-demand source snapshot.

Both AI surfaces use the compact line-oriented request frame. The OpenRouter HTTP
envelope and response schema remain JSON. Their edit execution stays
surface-specific: the graphical editor returns one proposal for approval, while
the TUI applies one contiguous atomic write batch through the live runtime. Both
resolve reads through the same catalog selectors and canonical source targets.

For live visual changes, the shared guidance preserves state and uses
`on_code_swap` when migration or immediate visual setup is required. It changes
only the minimum state, such as player position or game phase, needed to make the
requested result visible immediately.

## Measured acceptance

The real graphical-editor controller and OpenRouter serializer were captured
against a loopback server using RootbeerMaze3's saved task and nightly-317 vendor
snapshot. No external AI call or project mutation occurred.

| UTF-8 bytes | Previous | Compact |
| --- | ---: | ---: |
| Project catalog | 14,551 | 2,645 |
| Initial model message | 18,433 | 4,591 |
| Complete HTTP body, including response schema | 22,811 | 7,152 |

The complete request passes a strict **10,000-byte** budget and the catalog passes
the **5,000-byte** budget. These are exact serialized byte counts, not token
estimates. All 300 source items remain available for on-demand reads; symbols
outside `src/main.stasis` are deferred behind their file links. Large existing task
histories or attached images can add payload beyond this initial text-only case.

A paid proposal-only acceptance used RootbeerMaze3's configured
`openai/gpt-oss-120b` through OpenRouter at medium reasoning. The model expanded
`f2` and `f6` together, then read `s53` and `s213` together, and proposed one
test-only edit on its third model turn. The host canonicalized the new-symbol
target, compiled the candidate in memory, ran the project tests, and terminated
without a redundant completion turn. The run took 8.629 seconds, used 9,967 input
tokens and 2,201 output tokens across all turns, and reported a cost of $0.005140.
The source fingerprint before and after the proposal-only run was identical.

Reproduce the byte capture with the
[desktop editor payload review](reviews/desktop-editor-payload-20260912.md), setting
`STASIS_EDITOR_PAYLOAD_MAX_BYTES=10000` with its project and output variables. The
test asserts the limit against the actual received HTTP bytes. The paid acceptance
test is opt-in through `STASIS_EDITOR_OPENROUTER_EFFECTIVE_PROJECT`; it never applies
the returned proposal.

Visual evidence: not applicable; the inspected artifact is the captured text catalog.

Theory gained: a stable file map plus entry symbols and a pair of real examples is
smaller and easier to traverse than a complete initial symbol list. Literal `f` and
`s` links preserve discovery, while staged compiler validation keeps unsupported
model syntax inside the repair loop.
