# Finite text coverage proof

`ProgramSnapshot::text_coverage()` is the compiler-owned input contract for a
future release font-subsetting step. This analysis does not modify, package, or
subset fonts. Packaging must retain each complete font unless the result is
`TextCoverageProof::Finite`.

## Contract

The proof runs over accepted HIR after the selected reachability policy has
removed unreachable functions. Calls are resolved through the compiler's
module-qualified call signature table, not by matching source spelling alone.
The serialized result is one of:

- `finite`: sorted Unicode scalar values, sorted logical font paths, and sorted
  evidence records pairing a resolved sink identity with its calling function
  and font path.
- `unknown`: sorted machine-readable reason records plus any font and sink
  evidence that was independently proven. A single unknown reason requires the
  complete font file to be retained.

The bounded finite lattice accepts string literals, immutable top-level string
constants, immutable local forwarding, and finite unions of helper returns and
parameters. The compiler-owned decimal chain
`ascii_clear -> ascii_push_i32 -> utf8_from_ascii` contributes `-` and `0`-`9`
only when the complete chain and its buffers are resolved to the checked stdlib
functions.

The proof is deliberately unknown for recursive text helpers, mutable text,
loop-mutated values, indexed or aliased text, arbitrary buffers, unrecognized
extern text producers, unresolved or ambiguous calls, and unknown font handles
or paths. It is also bounded to 4,096 distinct Unicode scalars. Adding a new
accepted case belongs in this compiler analysis; packaging must not infer a
larger proof from source text or runtime behavior.

Release snapshots analyze only release-reachable HIR, so unreachable dynamic
text does not poison a finite release result. Development snapshots may be more
conservative because development-only roots can remain reachable.

## Tool and font metadata boundary

Task #653 may consume a finite result with optional build-time
`fonttools==4.60.2`. That exact version is the reproducibility pin; it is not a
runtime dependency. If the pinned distribution is absent, fails to load, or
cannot reproduce the requested transformation, the deterministic behavior is
to retain the complete original font and report the fallback. No substitute
version may be selected implicitly.

Before writing any transformed font, later tooling must inspect its name table
and licensing metadata. A font declaring a Reserved Font Name (RFN) may be
transformed only when the license permits modification and every reserved name
is replaced consistently in the required family, subfamily, full-name,
PostScript-name, and unique-identifier records. The output must be re-opened and
validated to prove that no prohibited reserved name remains. Ambiguous,
missing, conflicting, or non-decodable metadata requires the complete-font
fallback. This repository stores neither transformed output nor a fontTools
dependency as part of this proof task.

## Current sinks

The proof recognizes resolved compiler graphics contracts for direct text
measurement, literal cached text runs, mutable UTF-8 cached text runs, and the
compiler-owned `draw_text` wrapper. Custom functions with similar names do not
inherit these semantics. New sinks must be added by resolved function or extern
identity with positive and conservative-negative snapshot coverage.

Visual evidence: not applicable; this is compiler metadata only and produces no
user-visible rendering change.
