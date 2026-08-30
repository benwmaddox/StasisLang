# Android preview render performance

The Android Workshop performance gate uses the canonical `samples/render_parity`
scene. It uses the canonical current gfx_cmd schema and
exercises every category transition through the real x86_64
JIT, the embedded GLES renderer, and three pixel-verified captures. Timing starts
inside `StasisPreviewRenderer.onDrawFrame`; work is not deferred outside the
measured action.

Run the bounded gate on the repository API 35 emulator after building with
`-RenderAcceptance`:

```powershell
mobile/android/test_render_emulator.ps1 -Headless -SkipBuild `
  -MaxRenderP50Millis 1.05 -MaxRenderP95Millis 8.94
```

The gate discards 60 warm-up frames, measures 180 frames, records total,
resource-preparation, and ordered-draw p50/p95, and writes device fingerprint,
AVD, package version, Git revision, and APK SHA-256 beside the screenshots and
logcat. The driver verifies the observed AVD and Android API and refuses dirty
tracked source, so the recorded revision identifies the tested APK inputs.
`tools/ci/verify_android_render_performance.py` rejects missing, duplicated,
unexpected, malformed, dirty-source, or out-of-budget evidence.

## 2026-08-12 baseline and result

Both runs used `Stasis_API_35`, API 35 x86_64 fingerprint
`google/sdk_gphone64_x86_64/emu64xa:15/AE3A.240806.043/12960925:userdebug/dev-keys`,
the same 640x360 parity scene, 60 warm-up frames, and 180 measured frames.

| Build | Total p50 | Total p95 | Resource p50/p95 | Draw p50/p95 | Draw calls |
| --- | ---: | ---: | ---: | ---: | ---: |
| `origin/main` baseline | 0.846 ms | 7.152 ms | 0.080 / 0.557 ms | 0.739 / 5.862 ms | 9 |
| cached-resource/pipeline result | 0.456 ms | 4.169 ms | 0.080 / 0.271 ms | 0.324 / 3.905 ms | 9 |

The emulator-normalized p50 ceiling is 1.05 ms: 25% above the observed baseline.
The 8.94 ms p95 ceiling uses the same margin but remains a scheduling tripwire,
not a physical-device claim. The result also returns near the historical 0.4 ms
device observation. Physical-device evidence may establish a tighter device-class
threshold later.

Profiling showed ordered GLES submission dominated, while resource preparation
was already small. The renderer therefore keeps all nine semantically required
draw calls and their order. It resolves each sprite/text resource once into fixed
per-frame arrays, then reuses that identity during submission. Adjacent runs keep
compatible shader/uniform/attribute-enable state, while client-buffer pointers
are rebound after every refill as GLES requires. No frame allocation, texture
upload, command reordering, or deferred draw work was introduced.
