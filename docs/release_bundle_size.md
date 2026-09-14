# Nightly bundle size

The nightly archives intentionally contain a complete offline toolchain. The
Windows archive is the useful baseline because it is the largest supported
bundle: `nightly-20260903-290` was approximately 111 MB compressed.

## Findings

The checked-in payload is small compared with the Windows build tools. A local
audit of the current source tree measured these uncompressed groups:

| Group | Bytes |
| --- | ---: |
| `src` | 136,974 |
| `samples` | 2,910,323 |
| `runtime` | 2,880,279 |
| `mobile` | 1,948,252 |
| `docs` | 15,392,506 |

Only the selected release documentation and `docs/knowledge` are copied into a
nightly archive; the large `docs/demos` and `docs/evidence` trees are not
shipped. The dominant Windows inputs are the required `clang-cl.exe` and
`lld-link.exe` compiler/linker tools, plus their license notices. They are
needed to compile and link per-project native AOT bridges without requiring a
host Visual Studio or Rust installation. The standard library, samples,
runtime sources, mobile shells, mobile network libraries, and knowledge library
are likewise part of the documented offline contract.

Removing any of those inputs would make the archive smaller only by changing
the standalone contract to require an external toolchain or to drop offline
native/mobile packaging. Replacing ZIP with a different Windows archive format
would also change the existing installer and resolver contract. No safe content
removal or compression change was found.

## Regression audit

`tools/audit_release_bundle.py` validates every required file and directory,
checks that the archive contains exactly the extracted bundle files, reports
per-file and per-top-level-directory uncompressed sizes, archive compressed
sizes where the format exposes them, duplicate content hashes, retained large
files and rationales, and the archive SHA-256. The nightly workflow runs it
after each Linux, Windows, and macOS archive is created.

The workflow currently enforces a 120 MiB compressed archive ceiling. The audit
prints the measured current size and SHA-256 into the build log/summary, so a
future reduction can be recorded as an exact before/after comparison without
inventing a digest for the historical approximate baseline.
