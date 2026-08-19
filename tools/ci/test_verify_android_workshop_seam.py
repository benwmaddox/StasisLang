import json
import tempfile
import unittest
from pathlib import Path

try:
    from .verify_android_workshop_seam import SeamError, _read_json, verify_log
except ImportError:
    from verify_android_workshop_seam import SeamError, _read_json, verify_log


MANIFEST = {"state_checksum": 2500, "workshop_command_trace": 3939026311, "render_contract_version": 5}
GOOD = """CompileReady: backend=cranelift-jit reload=InitialCompile status=0 functions=7 compile_us=12 manifest=x
Stasis Workshop IT-025: {"schema":"stasis.workshop_seam.v1","test_id":"IT-025","event":"frame","jni_version":65542,"rust_bridge_version":"0.1.0","render_version":5,"state_checksum":2500,"command_trace":3939026311,"frame_token":1,"fallback":0,"stub":0}
Stasis Workshop IT-025 GLES: {"schema":"stasis.workshop_seam.v1","test_id":"IT-025","event":"present","count":1,"frame_token":1}
Stasis Workshop IT-025: {"schema":"stasis.workshop_seam.v1","test_id":"IT-025","event":"frame","jni_version":65542,"rust_bridge_version":"0.1.0","render_version":5,"state_checksum":2500,"command_trace":3939026311,"frame_token":77,"fallback":0,"stub":0}
Stasis Workshop IT-025 GLES: {"schema":"stasis.workshop_seam.v1","test_id":"IT-025","event":"present","count":30,"frame_token":77}
RenderAcceptanceFrame: count=1 frame_token=1
RenderAcceptanceFrame: count=30 frame_token=77
Stasis Workshop IT-026: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"buffer_abi","status":"passed","descriptor":{"lanes":[{"lane":"i32","bytes":138432,"alignment":4},{"lane":"f32","bytes":500240,"alignment":4},{"lane":"u8","bytes":65536,"alignment":1}]},"valid_guards_intact":true,"all_invalid_unchanged":true,"valid_calls":1,"invalid_calls":18}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"short_i32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"i32","reason":"capacity","expected":138432,"actual":138431}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"short_f32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"f32","reason":"capacity","expected":500240,"actual":500239}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"short_u8","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"u8","reason":"capacity","expected":65536,"actual":65535}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"oversized_i32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"i32","reason":"capacity","expected":138432,"actual":138433}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"oversized_f32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"f32","reason":"capacity","expected":500240,"actual":500241}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"oversized_u8","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"u8","reason":"capacity","expected":65536,"actual":65537}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"swapped_i32_f32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"i32","reason":"capacity","expected":138432,"actual":500240}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"wrong_order_i32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"i32","reason":"byte_order","expected":"native","actual":"non_native"}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"wrong_order_f32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"f32","reason":"byte_order","expected":"native","actual":"non_native"}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"wrong_order_u8","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"u8","reason":"byte_order","expected":"native","actual":"non_native"}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"heap_i32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"i32","reason":"not_direct","expected":138432,"actual":-1}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"heap_f32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"f32","reason":"not_direct","expected":500240,"actual":-1}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"heap_u8","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"u8","reason":"not_direct","expected":65536,"actual":-1}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"null_i32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"i32","reason":"null_buffer","expected":138432,"actual":-1}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"null_f32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"f32","reason":"null_buffer","expected":500240,"actual":-1}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"null_u8","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"u8","reason":"null_buffer","expected":65536,"actual":-1}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"misaligned_i32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"i32","reason":"alignment","expected":4,"actual":1}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"misaligned_f32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"f32","reason":"alignment","expected":4,"actual":1}}
Stasis Workshop IT-027 GLES: {"schema":"stasis.workshop_touch_roundtrip.v1","test_id":"IT-027","event":"present","frame_token":78,"trace":111,"rect_count":2,"order_count":11,"marker":{"active":true,"x":152,"y":82,"w":16,"h":16,"r":1.0,"g":0.65,"b":0.08,"a":1.0}}
Stasis Workshop IT-027 case: {"schema":"stasis.workshop_touch_roundtrip.v1","test_id":"IT-027","event":"case","status":"passed","phase":"down","sequence":1,"input":{"x":160,"y":90,"active":1,"action":0},"guest":{"x":160,"y":90,"dx":0,"dy":0,"x_norm_x1000":250,"y_norm_x1000":250,"active":1,"down_edge":1,"up_edge":0,"marker_active":1,"checksum":6466},"render":{"trace":111,"frame_token":78,"marker":{"active":true,"x":152,"y":82,"w":16,"h":16,"r":1.0,"g":0.65,"b":0.08,"a":1.0}},"gles_presented":true,"gles_frame_token":78,"java_only":false}
Stasis Workshop IT-027 GLES: {"schema":"stasis.workshop_touch_roundtrip.v1","test_id":"IT-027","event":"present","frame_token":79,"trace":112,"rect_count":2,"order_count":11,"marker":{"active":true,"x":312,"y":172,"w":16,"h":16,"r":1.0,"g":0.65,"b":0.08,"a":1.0}}
Stasis Workshop IT-027 case: {"schema":"stasis.workshop_touch_roundtrip.v1","test_id":"IT-027","event":"case","status":"passed","phase":"move","sequence":2,"input":{"x":320,"y":180,"active":1,"action":2},"guest":{"x":320,"y":180,"dx":160,"dy":90,"x_norm_x1000":500,"y_norm_x1000":500,"active":1,"down_edge":0,"up_edge":0,"marker_active":1,"checksum":14307},"render":{"trace":112,"frame_token":79,"marker":{"active":true,"x":312,"y":172,"w":16,"h":16,"r":1.0,"g":0.65,"b":0.08,"a":1.0}},"gles_presented":true,"gles_frame_token":79,"java_only":false}
Stasis Workshop IT-027 GLES: {"schema":"stasis.workshop_touch_roundtrip.v1","test_id":"IT-027","event":"present","frame_token":80,"trace":113,"rect_count":2,"order_count":11,"marker":{"active":true,"x":392,"y":217,"w":16,"h":16,"r":1.0,"g":0.65,"b":0.08,"a":1.0}}
Stasis Workshop IT-027 case: {"schema":"stasis.workshop_touch_roundtrip.v1","test_id":"IT-027","event":"case","status":"passed","phase":"up","sequence":3,"input":{"x":400,"y":225,"active":0,"action":1},"guest":{"x":400,"y":225,"dx":80,"dy":45,"x_norm_x1000":625,"y_norm_x1000":625,"active":0,"down_edge":0,"up_edge":1,"marker_active":1,"checksum":16813},"render":{"trace":113,"frame_token":80,"marker":{"active":true,"x":392,"y":217,"w":16,"h":16,"r":1.0,"g":0.65,"b":0.08,"a":1.0}},"gles_presented":true,"gles_frame_token":80,"java_only":false}
Stasis Workshop IT-027: {"schema":"stasis.workshop_touch_roundtrip.v1","test_id":"IT-027","event":"touch_roundtrip","status":"passed","phases":3,"ordered":true,"unique":true,"java_motion_events":3,"jni_jit_frames":3,"gles_presented_frames":3,"java_only":false}
Stasis Workshop IT-028 GLES: {"schema":"stasis.workshop_hot_edit.v1","test_id":"IT-028","event":"present","frame_token":81,"trace":114,"rect_count":2,"order_count":11,"marker":{"active":true,"x":112.0,"y":48.0,"w":24.0,"h":24.0,"r":0.2,"g":0.9,"b":0.95,"a":1.0}}
Stasis Workshop IT-028 case: {"schema":"stasis.workshop_hot_edit.v1","test_id":"IT-028","event":"case","status":"passed","phase":"baseline","sequence":1,"runtime":{"status":"RuntimeStateReady","generation":1,"source_fingerprint":"1111111111111111"},"guest":{"tick_revision":1,"render_revision":1,"state_counter":1},"render":{"trace":114,"frame_token":81,"rect_count":2,"marker":{"active":true,"x":112.0,"y":48.0,"w":24.0,"h":24.0,"r":0.2,"g":0.9,"b":0.95,"a":1.0}},"gles_presented":true,"gles_frame_token":81,"java_only":false,"fallback":0,"stub":0}
Stasis Workshop IT-028 GLES: {"schema":"stasis.workshop_hot_edit.v1","test_id":"IT-028","event":"present","frame_token":82,"trace":115,"rect_count":2,"order_count":11,"marker":{"active":true,"x":176.0,"y":48.0,"w":24.0,"h":24.0,"r":0.2,"g":0.9,"b":0.95,"a":1.0}}
Stasis Workshop IT-028 case: {"schema":"stasis.workshop_hot_edit.v1","test_id":"IT-028","event":"case","status":"passed","phase":"published","sequence":2,"runtime":{"status":"RuntimeStateReady","generation":2,"source_fingerprint":"2222222222222222"},"guest":{"tick_revision":2,"render_revision":2,"state_counter":2},"render":{"trace":115,"frame_token":82,"rect_count":2,"marker":{"active":true,"x":176.0,"y":48.0,"w":24.0,"h":24.0,"r":0.2,"g":0.9,"b":0.95,"a":1.0}},"gles_presented":true,"gles_frame_token":82,"java_only":false,"fallback":0,"stub":0}
Stasis Workshop IT-028 GLES: {"schema":"stasis.workshop_hot_edit.v1","test_id":"IT-028","event":"present","frame_token":83,"trace":115,"rect_count":2,"order_count":11,"marker":{"active":true,"x":176.0,"y":48.0,"w":24.0,"h":24.0,"r":0.2,"g":0.9,"b":0.95,"a":1.0}}
Stasis Workshop IT-028 case: {"schema":"stasis.workshop_hot_edit.v1","test_id":"IT-028","event":"case","status":"passed","phase":"post_invalid","sequence":3,"runtime":{"status":"RuntimeStateReady","generation":2,"source_fingerprint":"2222222222222222"},"guest":{"tick_revision":2,"render_revision":2,"state_counter":3},"render":{"trace":115,"frame_token":83,"rect_count":2,"marker":{"active":true,"x":176.0,"y":48.0,"w":24.0,"h":24.0,"r":0.2,"g":0.9,"b":0.95,"a":1.0}},"gles_presented":true,"gles_frame_token":83,"java_only":false,"fallback":0,"stub":0}
Stasis Workshop IT-028: {"schema":"stasis.workshop_hot_edit.v1","test_id":"IT-028","event":"hot_edit","status":"passed","ordered":true,"unique":true,"atomic":true,"hook_source_line":40,"invalid_compile":{"ok":false,"kind":"compile_error","diagnostic":{"file":"src/main.stasis","line":40,"column":10,"end_line":40,"end_column":22,"symbol":"on_code_swap","message":"unknown call target 'IT028_missing_target'"}},"restore_receipt":{"status":"NoChange","compile":"CompileReady: backend=cranelift-jit reload=NoChange status=0 functions=10 compile_us=12 manifest=build/native_compile_manifest.txt"},"cleanup_receipt":{"status":"Restored","compile":"CompileReady: backend=cranelift-jit reload=FastReload status=0 functions=10 compile_us=13 manifest=build/native_compile_manifest.txt","frame":{"status":"passed","runtime":{"generation":3,"source_fingerprint":"1111111111111111"},"render":{"marker":{"active":false}},"java_only":false,"fallback":0,"stub":0}}}
"""


class WorkshopSeamTests(unittest.TestCase):
    def test_accepts_complete_single_frame_proof(self):
        result = verify_log(GOOD, MANIFEST)
        self.assertEqual(result["compile_functions"], 7)
        self.assertEqual(result["presented_frames"], 30)
        self.assertEqual(result["it028"]["test_id"], "IT-028")

    def test_rejects_wrong_guest_state(self):
        with self.assertRaisesRegex(SeamError, "state_checksum"):
            verify_log(GOOD.replace('"state_checksum":2500', '"state_checksum":2501'), MANIFEST)

    def test_rejects_missing_marker_or_presentation(self):
        with self.assertRaisesRegex(SeamError, "IT-025"):
            verify_log(GOOD.replace("Stasis Workshop IT-025:", "Other:"), MANIFEST)
        with self.assertRaisesRegex(SeamError, "presentation"):
            verify_log(GOOD.replace("count=30", "count=2"), MANIFEST)

    def test_rejects_native_and_gles_token_mismatch(self):
        with self.assertRaisesRegex(SeamError, "token"):
            verify_log(GOOD.replace('"count":30,"frame_token":77', '"count":30,"frame_token":88'), MANIFEST)

    def test_rejects_stable_frame_log_token_mismatch(self):
        with self.assertRaisesRegex(SeamError, "RenderAcceptanceFrame"):
            verify_log(GOOD.replace("count=30 frame_token=77", "count=30 frame_token=999"), MANIFEST)

    def test_reads_powershell_utf8_bom_json(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "metadata.json"
            path.write_text('\ufeff{"git_revision":"abc"}', encoding="utf-8")
            self.assertEqual(_read_json(path)["git_revision"], "abc")

    def test_rejects_missing_it026_marker(self):
        with self.assertRaisesRegex(SeamError, "IT-026"):
            verify_log(GOOD.split("Stasis Workshop IT-026:")[0], MANIFEST)

    def test_rejects_incomplete_it026_scenarios(self):
        with self.assertRaisesRegex(SeamError, "scenario"):
            verify_log(GOOD.replace('"name":"null_i32"', '"name":"other"'), MANIFEST)

    def test_rejects_missing_it026_canary_proof(self):
        with self.assertRaisesRegex(SeamError, "guard"):
            verify_log(GOOD.replace('"valid_guards_intact":true', '"valid_guards_intact":false'), MANIFEST)

    def test_rejects_wrong_it026_error_lane(self):
        with self.assertRaisesRegex(SeamError, "lane/reason"):
            verify_log(GOOD.replace('"lane":"i32","reason":"byte_order"', '"lane":"all","reason":"byte_order"'), MANIFEST)

    def test_rejects_wrong_it026_expected_actual(self):
        with self.assertRaisesRegex(SeamError, "capacity proof"):
            verify_log(GOOD.replace('"actual":138431', '"actual":138430'), MANIFEST)

    def test_rejects_duplicate_it026_case(self):
        short_i32 = next(line for line in GOOD.splitlines() if '"name":"short_i32"' in line)
        short_f32 = next(line for line in GOOD.splitlines() if '"name":"short_f32"' in line)
        with self.assertRaisesRegex(SeamError, "scenarios mismatch"):
            verify_log(GOOD.replace(short_f32, short_i32), MANIFEST)

    def test_rejects_duplicate_it026_descriptor_lane(self):
        with self.assertRaisesRegex(SeamError, "ordered and unique"):
            verify_log(GOOD.replace('"lane":"f32","bytes":500240',
                                    '"lane":"i32","bytes":500240', 1), MANIFEST)

    def test_rejects_multiple_it026_summaries(self):
        summary = next(line for line in GOOD.splitlines() if "Stasis Workshop IT-026:" in line)
        with self.assertRaisesRegex(SeamError, "exactly one IT-026 summary"):
            verify_log(GOOD + "\n" + summary, MANIFEST)

    def test_rejects_it026_case_before_summary(self):
        case = next(line for line in GOOD.splitlines() if "Stasis Workshop IT-026 case:" in line)
        first_line, remainder = GOOD.split("\n", 1)
        with self.assertRaisesRegex(SeamError, "before its summary"):
            verify_log(first_line + "\n" + case + "\n" + remainder, MANIFEST)

    def test_rejects_missing_it027_proof(self):
        with self.assertRaisesRegex(SeamError, "IT-027"):
            verify_log(GOOD.split("Stasis Workshop IT-027 case:")[0], MANIFEST)

    def test_rejects_it027_token_mismatch(self):
        with self.assertRaisesRegex(SeamError, "matching GLES"):
            verify_log(GOOD.replace('"gles_frame_token":79', '"gles_frame_token":99'), MANIFEST)

    def test_rejects_it027_wrong_delta(self):
        with self.assertRaisesRegex(SeamError, "edge/delta"):
            verify_log(GOOD.replace('"dx":160,"dy":90', '"dx":161,"dy":90'), MANIFEST)

    def test_rejects_it027_input_action_mismatch(self):
        with self.assertRaisesRegex(SeamError, "input action/coordinates"):
            verify_log(GOOD.replace('"phase":"move","sequence":2,"input":{"x":320,"y":180,"active":1,"action":2}',
                                    '"phase":"move","sequence":2,"input":{"x":320,"y":180,"active":1,"action":1}'),
                       MANIFEST)

    def test_rejects_missing_it027_gles_marker(self):
        marker = next(line for line in GOOD.splitlines() if "IT-027 GLES" in line)
        with self.assertRaisesRegex(SeamError, "exactly 3 IT-027 GLES"):
            verify_log(GOOD.replace(marker, ""), MANIFEST)

    def test_rejects_it027_case_after_summary(self):
        case = next(line for line in GOOD.splitlines() if "IT-027 case" in line)
        summary = next(line for line in GOOD.splitlines() if "Stasis Workshop IT-027:" in line)
        reordered = GOOD.replace(case, "").replace(summary, summary + "\n" + case)
        with self.assertRaisesRegex(SeamError, "precede the IT-027 summary"):
            verify_log(reordered, MANIFEST)

    def test_rejects_it027_trace_marker_mismatch(self):
        with self.assertRaisesRegex(SeamError, "traces|GLES marker"):
            verify_log(GOOD.replace('"trace":111', '"trace":112'), MANIFEST)

    def test_rejects_it027_marker_color_mismatch(self):
        with self.assertRaisesRegex(SeamError, "geometry/color|geometry mismatch"):
            verify_log(GOOD.replace('"g":0.65', '"g":0.64', 1), MANIFEST)

    def test_rejects_it028_generation_without_one_publication_boundary(self):
        with self.assertRaisesRegex(SeamError, "generation"):
            verify_log(GOOD.replace('"generation":2,"source_fingerprint":"2222',
                                    '"generation":3,"source_fingerprint":"2222', 1), MANIFEST)

    def test_rejects_it028_invalid_compile_raw_text(self):
        with self.assertRaisesRegex(SeamError, "forbidden|structured diagnostic"):
            verify_log(GOOD.replace('"invalid_compile":{"ok":false',
                                    '"invalid_compile":{"raw":"CompileError", "ok":false'), MANIFEST)

    def test_rejects_it028_missing_gles_marker(self):
        marker = next(line for line in GOOD.splitlines() if "IT-028 GLES" in line)
        with self.assertRaisesRegex(SeamError, "exactly 3 IT-028"):
            verify_log(GOOD.replace(marker, "", 1), MANIFEST)

    def test_rejects_it028_trace_mismatch(self):
        with self.assertRaisesRegex(SeamError, "trace"):
            verify_log(GOOD.replace('"trace":115,"frame_token":82',
                                    '"trace":116,"frame_token":82', 1), MANIFEST)

    def test_rejects_it028_unmigrated_guest_state(self):
        with self.assertRaisesRegex(SeamError, "migrated"):
            verify_log(GOOD.replace('"state_counter":2', '"state_counter":1', 1), MANIFEST)

    def test_rejects_grouped_it028_presentations_and_cases(self):
        lines = GOOD.splitlines()
        case_lines = [line for line in lines if "Stasis Workshop IT-028 case:" in line]
        remaining = [line for line in lines if "Stasis Workshop IT-028 case:" not in line]
        summary_index = next(index for index, line in enumerate(remaining)
                             if line.startswith("Stasis Workshop IT-028: "))
        grouped = remaining[:summary_index] + case_lines + remaining[summary_index:]
        with self.assertRaisesRegex(SeamError, "interleaved"):
            verify_log("\n".join(grouped), MANIFEST)

    def test_rejects_it028_diagnostic_line_column_span_or_message(self):
        replacements = [
            ('"line":40', '"line":41'),
            ('"column":10', '"column":11'),
            ('"end_line":40', '"end_line":41'),
            ('"end_column":22', '"end_column":23'),
            ('"file":"src/main.stasis"', '"file":"src/other.stasis"'),
            ('"symbol":"on_code_swap"', '"symbol":"other"'),
            ('"message":"unknown call target \'IT028_missing_target\'"',
             '"message":"other diagnostic"'),
        ]
        for before, after in replacements:
            with self.subTest(field=before):
                with self.assertRaisesRegex(SeamError, "diagnostic"):
                    verify_log(GOOD.replace(before, after, 1), MANIFEST)

    def test_rejects_it028_cleanup_failure(self):
        with self.assertRaisesRegex(SeamError, "forbidden"):
            verify_log(GOOD + "\nIT-028 cleanup failed: StateError: unavailable\n", MANIFEST)


if __name__ == "__main__":
    unittest.main()
