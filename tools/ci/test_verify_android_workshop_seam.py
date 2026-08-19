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


if __name__ == "__main__":
    unittest.main()
