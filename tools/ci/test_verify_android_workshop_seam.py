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
"""


class WorkshopSeamTests(unittest.TestCase):
    def test_accepts_complete_single_frame_proof(self):
        result = verify_log(GOOD, MANIFEST)
        self.assertEqual(result["compile_functions"], 7)
        self.assertEqual(result["presented_frames"], 30)

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


if __name__ == "__main__":
    unittest.main()
