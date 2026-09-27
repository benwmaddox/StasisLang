# Finite text coverage proof

`ProgramSnapshot::text_coverage()` is the compiler-owned input contract for the
optional release font-subsetting step. The analysis itself does not modify,
package, or subset fonts. Packaging retains each complete font unless the
result is `TextCoverageProof::Finite` and that font is explicitly opted in.

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

## Release packaging opt-in

Font subsetting is disabled by default and is never used by development/JIT or
`--development-build` packages. A manifest-v2 project opts individual TTF files
in with explicit legal metadata:

```json
{
  "release": {
    "font_subsetting": {
      "fonts": [
        {
          "path": "assets/fonts/ui.ttf",
          "license_path": "assets/fonts/OFL.txt",
          "modification_permitted": true,
          "reserved_names": ["Example Reserved Name"],
          "replacement_family": "My Game UI"
        }
      ]
    }
  }
}
```

`path` and `license_path` must remain under `assets/`; only `.ttf` inputs are
eligible in this version. `modification_permitted` is an explicit project-owner
assertion, not a license inference. `reserved_names` must list every Reserved
Font Name, including an explicit empty list when there are none. A nonempty
list requires a non-reserved `replacement_family`. Ambiguous, incomplete, or
invalid metadata keeps the original font. Every release package that stages the opted-in
font also copies the validated notice to `font_licenses/<license-sha256>.txt`,
whether that font is subset or retained.

The build-time driver also requires decodable source family/subfamily/full,
unique, PostScript, and embedded license name records. An embedded Reserved
Font Name declaration not covered by `reserved_names`, or a configured name
not evidenced by the source metadata, retains the complete font.

Release packaging uses the compiler proof's conservative union of scalars for
each evidenced configured font. If the same font is selected by
`web.loading_font`, the static loading title is added before subsetting. A font
not present in resolved proof evidence remains complete.

## Tool and font metadata boundary

Release packaging consumes a finite result with the embedded driver in
`tools/font-subsetting/subset_font.py` and optional build-time
`fonttools==4.60.2`, pinned by `tools/font-subsetting/requirements.txt`. The
driver is embedded in the Stasis build tool; Python and fontTools are never
runtime dependencies of the packaged game. If the exact pinned distribution is
absent, fails to load, or cannot reproduce the requested transformation, the
deterministic behavior is to retain the complete original font and report the
fallback. No substitute version is selected implicitly.

Before writing any transformed font, later tooling must inspect its name table
and licensing metadata. A font declaring a Reserved Font Name (RFN) may be
transformed only when the license permits modification and every reserved name
is replaced consistently in the required family, subfamily, full-name,
PostScript-name, and unique-identifier records. The output must be re-opened and
validated to prove that no prohibited reserved name remains. Ambiguous,
missing, conflicting, or non-decodable metadata requires the complete-font
fallback. Processing preserves hinting, metrics, composite/layout closure, and
the missing-glyph outline. It does not change font format.

Each configured release writes `stasis_font_subsets.json`. The deterministic
report records the decision/reason, exact tool and options identity, sorted
coverage, source/output/license hashes, raw and DEFLATE sizes, legal naming
metadata, and cache outcome. The cache key covers the source hash, coverage,
tool/options identity, and legal/name transformation. Transformed asset hashes
and `stasis_asset_package.json` are regenerated from the final staged bytes. If
the result has no useful raw and compressed-size saving, the original is kept.

## Current sinks

The proof recognizes resolved compiler graphics contracts for direct text
measurement, literal cached text runs, mutable UTF-8 cached text runs, and the
compiler-owned `draw_text` wrapper. Custom functions with similar names do not
inherit these semantics. New sinks must be added by resolved function or extern
identity with positive and conservative-negative snapshot coverage.

Visual evidence: not applicable; this is compiler metadata only and produces no
user-visible rendering change.
