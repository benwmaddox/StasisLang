# Android numeric-text seam

This sample formats a fixed, ordered corpus into the caller-owned global
`numeric_text_receipt`. The test-only Android shell reads that registered `u8`
array after the first packaged AOT frame and logs its literal bytes. The host
runner compares every case ID, status, and formatted value with
`android_seam_expectations.json`.

The sample seeds nonempty transactionality buffers with explicit ASCII bytes.
Android packaged AOT currently observes literal-backed `ascii_copy` sources as
empty in this global-buffer setup (MaddoxTasks #782). That setup limitation is
separate from the numeric formatter exercised by the receipt.

The x86_64 emulator lane is executable byte-parity evidence. The arm64 CI lane
only proves packaging, linking, and native-library provenance.
