import copy
import tempfile
import unittest
from pathlib import Path

from tools.ci import check_runtime_abi_contract as contract


class RuntimeAbiContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.sources = {
            path: (contract.ROOT / path).read_text(encoding="utf-8")
            for path in contract.REQUIRED
        }

    def run_with(self, path, old, new):
        overlays = copy.deepcopy(self.sources)
        self.assertIn(old, overlays[path])
        overlays[path] = overlays[path].replace(old, new, 1)
        return contract.check(overlays=overlays)

    def test_source_discovery_ignores_generated_copies(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "src" / "main.stasis"
            source.parent.mkdir(parents=True)
            source.write_text("function main(): i32 { return 0; }\n", encoding="utf-8")
            for ignored in ("build", "dist", "target", ".stasis_cache"):
                generated = root / ignored / "copy.stasis"
                generated.parent.mkdir(parents=True)
                generated.write_text("generated", encoding="utf-8")

            self.assertEqual(contract.repository_stasis_sources(root), [source])

    def test_repository_contract_passes_and_emits_evidence(self):
        failures, evidence = contract.check(overlays=self.sources)
        self.assertEqual([], failures)
        self.assertEqual("stasis.seam_test.v1", evidence["schema"])
        self.assertEqual("IT-001", evidence["test_id"])
        self.assertEqual("passed", evidence["status"])
        self.assertGreater(evidence["checks"], 100)

    def test_stasis_capacity_drift_names_both_sides(self):
        failures, _ = self.run_with(
            contract.GFX_CMD,
            "global gfx_cmd_i32: i32[34608];",
            "global gfx_cmd_i32: i32[34609];",
        )
        message = "\n".join(map(str, failures))
        self.assertIn("producer=runtime/stasis_render_contract.h", message)
        self.assertIn("consumer=src/stdlib/internal/gfx_cmd.stasis", message)
        self.assertIn("field=gfx_cmd_i32.length", message)
        self.assertIn("expected=34608 actual=34609", message)

    def test_java_version_drift_is_rejected(self):
        failures, _ = self.run_with(
            contract.JAVA_RENDERER,
            "static final int RENDER_VERSION = 5;",
            "static final int RENDER_VERSION = 6;",
        )
        self.assertTrue(any(failure.field == "STASIS_RENDER_CURRENT_VERSION" for failure in failures))

    def test_web_magic_drift_reports_contract_provenance(self):
        failures, _ = self.run_with(
            contract.WEB,
            "const GFX_CMD_MAGIC = 0x47584631;",
            "const GFX_CMD_MAGIC = 0x47584632;",
        )
        message = "\n".join(map(str, failures))
        self.assertIn("producer=runtime/stasis_render_contract.h", message)
        self.assertIn("consumer=runtime/web/game.js", message)
        self.assertIn("field=STASIS_RENDER_V2_MAGIC", message)
        self.assertIn("expected=1196967473 actual=1196967474", message)

    def test_web_layout_drift_reports_field_and_values(self):
        failures, _ = self.run_with(
            contract.WEB,
            "const GFX_F_TEXT_BASE = 112772;",
            "const GFX_F_TEXT_BASE = 112773;",
        )
        failure = next(
            failure for failure in failures if failure.field == "STASIS_RENDER_F_TEXT_BASE"
        )
        self.assertEqual(112772, failure.expected)
        self.assertEqual(112773, failure.actual)

    def test_web_current_version_capacity_stride_and_offset_drift(self):
        mutations = (
            ("const GFX_CMD_CURRENT_VERSION = GFX_CMD_V5_VERSION;",
             "const GFX_CMD_CURRENT_VERSION = GFX_CMD_V4_VERSION;",
             "STASIS_RENDER_CURRENT_VERSION", 5, 4),
            ("const GFX_MAX_TEXT = 2048;", "const GFX_MAX_TEXT = 2047;",
             "STASIS_RENDER_MAX_TEXT", 2048, 2047),
            ("const GFX_SPRITE_STRIDE_F32 = 8;", "const GFX_SPRITE_STRIDE_F32 = 7;",
             "STASIS_RENDER_SPRITE_F32_STRIDE", 8, 7),
            ("const GFX_I_ORDER_BASE = 18464;", "const GFX_I_ORDER_BASE = 18465;",
             "STASIS_RENDER_I_ORDER_BASE", 18464, 18465),
        )
        for old, new, field, expected, actual in mutations:
            failures, _ = self.run_with(contract.WEB, old, new)
            failure = next(failure for failure in failures if failure.field == field)
            self.assertEqual(expected, failure.expected)
            self.assertEqual(actual, failure.actual)
            self.assertEqual("runtime/stasis_render_contract.h", failure.producer)
            self.assertEqual("runtime/web/game.js", failure.consumer)

    def test_provenance_current_version_drift_reports_values(self):
        mutations = (
            (contract.PACKAGE_PROVENANCE, "CURRENT_COMMAND_BUFFER_VERSION = 5",
             "CURRENT_COMMAND_BUFFER_VERSION = 6", "tools/verify_package_provenance.py"),
            (contract.TOOLCHAIN, "GFX_CMD_CURRENT_VERSION: i64 = 5",
             "GFX_CMD_CURRENT_VERSION: i64 = 6", "apps/stasis/src/toolchain_cli.rs"),
        )
        for path, old, new, consumer in mutations:
            failures, _ = self.run_with(path, old, new)
            failure = next(
                failure for failure in failures
                if failure.field == "STASIS_RENDER_CURRENT_VERSION"
                and failure.consumer == consumer
            )
            self.assertEqual(5, failure.expected)
            self.assertEqual(6, failure.actual)

    def test_rust_offset_drift_is_rejected(self):
        failures, _ = self.run_with(
            contract.DYNLOAD,
            "const STASIS_RENDER_ORDER_BASE: usize = 18_464;",
            "const STASIS_RENDER_ORDER_BASE: usize = 18_465;",
        )
        self.assertTrue(any(failure.field == "STASIS_RENDER_I_ORDER_BASE" for failure in failures))

    def test_android_host_writer_drift_is_rejected(self):
        failures, _ = self.run_with(
            contract.ANDROID,
            "host_i32[30] = session.display_generation;",
            "host_i32[29] = session.display_generation;",
        )
        self.assertTrue(any(failure.field == "HOST_I_DISPLAY_GENERATION" for failure in failures))

    def test_generated_aot_registration_drift_is_rejected(self):
        failures, _ = self.run_with(
            contract.AOT,
            "gfx_cmd_f32, 125060);",
            "gfx_cmd_f32, 125059);",
        )
        self.assertTrue(any(failure.field == "gfx_cmd_f32.registration_length" for failure in failures))

    def test_jni_must_reference_canonical_capacity_macros(self):
        overlays = copy.deepcopy(self.sources)
        overlays[contract.JNI] = overlays[contract.JNI].replace("STASIS_RENDER_U8_COUNT", "65536")
        failures, _ = contract.check(overlays=overlays)
        self.assertTrue(any(failure.field == "STASIS_RENDER_U8_COUNT" for failure in failures))

    def test_render_descriptor_must_keep_canonical_lane_expressions(self):
        mutations = {
            "i32": ("sizeof(int32_t), _Alignof(int32_t)", "sizeof(float), _Alignof(int32_t)"),
            "f32": ("sizeof(float), _Alignof(float)", "sizeof(int32_t), _Alignof(float)"),
            "u8": ("sizeof(uint8_t), _Alignof(uint8_t)", "sizeof(uint16_t), _Alignof(uint8_t)"),
        }
        for lane, (canonical, mutated) in mutations.items():
            overlays = copy.deepcopy(self.sources)
            overlays[contract.RENDER_HEADER] = overlays[contract.RENDER_HEADER].replace(canonical, mutated, 1)
            failures, _ = contract.check(overlays=overlays)
            self.assertTrue(any(failure.field == f"descriptor.{lane}" for failure in failures))

    def test_jni_descriptor_initializer_must_execute_canonical_macro(self):
        overlays = copy.deepcopy(self.sources)
        invocation = "STASIS_RENDER_BUFFER_DESCRIPTORS(STASIS_JNI_FRAME_DESCRIPTOR)"
        overlays[contract.JNI] = overlays[contract.JNI].replace(
            invocation, f"/* {invocation} */", 1)
        failures, _ = contract.check(overlays=overlays)
        self.assertTrue(any(failure.field == "STASIS_RENDER_BUFFER_DESCRIPTORS.initializer"
                            for failure in failures))


if __name__ == "__main__":
    unittest.main()
