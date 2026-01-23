# Tasks

## Diagnostics (Elm-like)

### Goals
- Errors explain: what happened, where, and what to do next.
- Same diagnostic quality in CLI, watch/hotswap, tests, and LSP.
- No silent errors. Prefer early semantic errors over backend/lowering errors.
- Fast failure: stop after `DiagnosticPolicy.MaxErrors` (currently 5), but keep the first errors maximally useful.

### Current gaps (to address)
- CLI printing (`Stasis.Cli/Program.cs:5108`) is "error: <message>" + caret, but has no structured hint/note, no codes, and no "did you mean" suggestions.
- Many diagnostics are one-line rules without fix guidance.
- LSP diagnostics are truncated to 5, but do not distinguish primary vs follow-on errors.

### Proposed diagnostic shape
- Add a richer diagnostic model in `Stasis.Compiler`:
  - `Code` (stable id, e.g. `STASIS1001`)
  - `Title` (short summary)
  - `Message` (1 sentence)
  - `Hint` (how to fix; may be null)
  - `Notes` (optional extra context)
  - `Labels` (1+ spans, each with short label text)
  - `Related` (optional locations; e.g. symbol definition)
  - `Severity` (Error/Warning; keep warnings rare and intentional)

### CLI rendering upgrades
- Replace `PrintDiagnostics` with an Elm-like renderer:
  - Header: `-- <TITLE> -------------------------------- <file>:<line>:<col>`
  - Show 1-3 lines of source context with line numbers.
  - Label the primary span and any secondary spans.
  - Print `Hint:` and `Note:` sections when present.
  - Print `Code:` at the end for searchability.
- When hitting the error cap:
  - Print the first 5 diagnostics, then a final line: `error: too many errors; stopping after 5 (fix the first error and retry).`

### High-value suggestions (implement first)
- Unknown function call:
  - Message: "Unknown function 'foo'."
  - Hint: show closest matches and/or remind that functions are declared with `function name(...): type { ... }`.
- Calling non-function:
  - Message: "'x' is not callable."
  - Hint: if `x` is a local/global, suggest removing `()` or renaming one of the symbols.
  - Note: include symbol kind (local/global/const) and its type.
- Unknown field:
  - Message: "Unknown field 'b' on struct 'S'."
  - Hint: show closest matching field names and list available fields when the struct is small.
  - Related: include the struct declaration location (so editors can jump).
- Member access on non-struct:
  - Message: "Member access requires a struct type; got 'i32'."
  - Hint: show the receiver expression type and suggest storing a struct reference (index) or using the correct value.

### LSP parity
- Map `Code` into LSP `Diagnostic.Code`, and `Title/Message/Hint` into `Diagnostic.Message` (Message + "\nHint: ...").
- Set `Diagnostic.Source = "stasis"` for all diagnostics.
- Preserve primary span for `Range`; additional labels go into `RelatedInformation` until we support full multi-range diagnostics.

### Tests (ensure we never regress)
- Add snapshot-style tests for key diagnostics:
  - Unknown field includes: struct name + hint (did-you-mean or field list).
  - Unknown function includes: function name + hint (did-you-mean).
  - Not callable includes: symbol kind/type + hint.
- Add LSP tests verifying:
  - The same invalid programs produce diagnostics with the same `Code`.
  - Messages contain `Hint:` for the above cases.
  - Truncation keeps the most relevant diagnostics (prefer semantic errors over cascades).

### Follow-ups / cleanup
- Fix encoding in `docs/error-messages.md` (currently contains non-ASCII mojibake); keep docs ASCII.
- Reference the diagnostic style doc from `docs/spec.md` and keep examples in sync.

