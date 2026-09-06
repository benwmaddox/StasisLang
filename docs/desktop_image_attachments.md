# Desktop image attachments

Image attachments belong to the task where they were added. File selection,
file drop, clipboard images, and live captures enter the same task-scoped
request flow. Adding an image does not authorize a provider upload. Select the
inline consent control before sending; each grant authorizes one request only.
Remove deletes the editor-owned copy. The original selected file is untouched.
At most eight images can be included in one request, across all input sources
and selection actions. Undo an inclusion to make room for another image.
Text-only messages retain previously completed image states.

PNG and JPEG inputs are limited to 16 MiB and 4096 pixels per dimension, with
bounded decoder allocation. Thumbnails fit within 512 by 512 pixels.
The editor retains a bounded thumbnail and a SHA-256 of the owned encoded bytes.
The provider boundary verifies those bytes again. Changed files are rejected,
not silently substituted. Credentials and multimodal provider envelopes stay
outside task state.
On Unix, session storage is created with owner-only directory (`0700`) and
file (`0600`) permissions. Storage initialization failures reject intake.

Unknown image capabilities fail closed. OpenRouter image support comes from
the exact configured model's reported input modalities. Refresh capability
metadata before selecting images for a newly configured model. A canceled or
failed request does not authorize a second image upload: select the image again
and send a new message. Text retries and reconnects do not replay attachments.
Recovery clears transient selection and consent.

## Validation

Run Cargo through the repository cache wrapper:

```text
python tools/cargo_cache.py run -- cargo test -p stasis_ai --lib
python tools/cargo_cache.py run -- cargo test -p stasis --bin stasis toolchain_cli::desktop_editor -- --test-threads=1
```

Live OpenRouter evidence is an explicit credentialed opt-in check. Deterministic
transport tests prove encoded payload bytes and capability gating against a local
server; they do not establish that a remote model received or understood pixels.
Never record API keys or raw provider request envelopes in task history or
evidence reports.

The credentialed check is:

```text
python tools/cargo_cache.py run -- cargo run -p stasis_ai --example openrouter_image_eval
```

Set `STASIS_RUN_OPENROUTER_IMAGE_EVAL=1`, `OPENROUTER_API_KEY`,
`STASIS_AI_MODEL`, `STASIS_OPENROUTER_IMAGE_PATH`, and
`STASIS_OPENROUTER_IMAGE_EXPECT` in the invoking environment. The last value
is a visible fact checked locally against the response; it is never disclosed
in the prompt. Use a distinctive image whose contents cannot be inferred from
its filename. The example prints its content hash and the response, or fails
if the model does not report image support or does not identify the fact.
