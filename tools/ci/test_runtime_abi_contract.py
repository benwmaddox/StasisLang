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

    def test_downstream_legacy_render_token_is_rejected(self):
        failures, _ = self.run_with(
            contract.RENDER_PARITY_TRACE,
            'function @extern("stasis_jit_render_trace")',
            'function @extern("stasis_jit_render_v2_trace")',
        )
        failure = next(
            failure for failure in failures
            if failure.field == "render_abi.legacy_token"
        )
        self.assertEqual("runtime/stasis_render_contract.h", failure.producer)
        self.assertEqual("samples/render_parity/trace.stasis", failure.consumer)

    def test_android_bridge_versioned_render_api_is_rejected(self):
        mutations = (
            (
                contract.ANDROID,
                "pub extern \"C\" fn stasis_android_bridge_run_render_frame(",
                "pub extern \"C\" fn stasis_android_bridge_run_tick_frame(",
            ),
            (
                contract.ANDROID,
                "pub extern \"C\" fn stasis_android_bridge_run_render_frame(",
                "pub extern \"C\" fn stasis_android_bridge_run_tick_frame_v2(",
            ),
            (
                contract.JNI,
                'dlsym(rust_bridge_api.handle, "stasis_android_bridge_run_render_frame")',
                'dlsym(rust_bridge_api.handle, "stasis_android_bridge_run_tick_frame")',
            ),
            (
                contract.JNI,
                'dlsym(rust_bridge_api.handle, "stasis_android_bridge_run_render_frame")',
                'dlsym(rust_bridge_api.handle, "stasis_android_bridge_run_tick_frame_v2")',
            ),
        )
        for path, current, legacy in mutations:
            with self.subTest(path=path):
                failures, _ = self.run_with(path, current, legacy)
                failure = next(
                    failure
                    for failure in failures
                    if failure.field == "render_abi.legacy_token"
                )
                self.assertEqual("runtime/stasis_render_contract.h", failure.producer)
                self.assertEqual(path.as_posix(), failure.consumer)

    def test_replay_trace_requires_current_capacities(self):
        failures, _ = self.run_with(
            contract.JIT_AOT_REPLAY_FIXTURE,
            "gfx_cmd_f32, 146564,",
            "gfx_cmd_f32, 125060,",
        )
        failure = next(
            failure for failure in failures
            if failure.field == "render_trace.current_capacities"
        )
        self.assertEqual("runtime/stasis_render_contract.h", failure.producer)

    def test_vscode_render_fixture_rejects_legacy_version_and_capacity(self):
        mutations = (
            ("gfx_cmd_i32[1] = 7;", "gfx_cmd_i32[1] = 6;", "STASIS_RENDER_VERSION"),
            (
                "global gfx_cmd_f32: f32[146564];",
                "global gfx_cmd_f32: f32[125060];",
                "gfx_cmd_f32.length",
            ),
        )
        for current, stale, field in mutations:
            with self.subTest(field=field):
                failures, _ = self.run_with(
                    contract.VSCODE_RENDER_FIXTURE, current, stale
                )
                failure = next(failure for failure in failures if failure.field == field)
                self.assertEqual("runtime/stasis_render_contract.h", failure.producer)
                self.assertEqual(
                    "vscode-stasis/test/fixture/src/main.stasis", failure.consumer
                )

    def test_manual_sprite_fixtures_reject_legacy_version_and_stride(self):
        mutations = (
            (
                contract.WINDOWS_LAUNCH_FIXTURE,
                "gfx_cmd_f32[80017] = 204.0;",
                "gfx_cmd_f32[80012] = 204.0;",
                "sprite_f32_stride",
            ),
            (
                contract.WORKSHOP_PREVIEW_ADAPTER,
                "let f_base: i32 = 80004 + index * 13;",
                "let f_base: i32 = 80004 + index * 8;",
                "sprite_f32_stride",
            ),
            (
                contract.WORKSHOP_PREVIEW_ADAPTER,
                "gfx_cmd_i32[1] = 7;",
                "gfx_cmd_i32[1] = 6;",
                "STASIS_RENDER_VERSION",
            ),
        )
        for path, current, stale, field in mutations:
            with self.subTest(path=path, field=field):
                failures, _ = self.run_with(path, current, stale)
                failure = next(failure for failure in failures if failure.field == field)
                self.assertEqual("runtime/stasis_render_contract.h", failure.producer)
                self.assertEqual(path.as_posix(), failure.consumer)

    def test_hot_swap_fixtures_require_complete_current_v7_header(self):
        mutations = (
            (contract.HOT_SWAP_V1_FIXTURE, "gfx_cmd_i32[1] = 7;", "gfx_cmd_i32[1] = 6;", "STASIS_RENDER_VERSION"),
            (contract.HOT_SWAP_V2_FIXTURE, "gfx_cmd_i32[24] = 0;", "gfx_cmd_i32[24] = 1;", "current_v7_header[24]"),
            (contract.HOT_SWAP_REJECT_FIXTURE, "gfx_cmd_i32[28] = 0;", "", "current_v7_header[28]"),
        )
        for path, current, stale, field in mutations:
            with self.subTest(path=path, field=field):
                failures, _ = self.run_with(path, current, stale)
                failure = next(failure for failure in failures if failure.field == field)
                self.assertEqual("runtime/stasis_render_contract.h", failure.producer)
                self.assertEqual(path.as_posix(), failure.consumer)

    def test_desktop_guest_trace_oracle_rejects_order_schema_and_consumer_drift(self):
        mutations = (
            (
                contract.DESKTOP,
                '\\"guest_trace\\":{guest_trace},\\"trace\\":{}',
                '\\"trace\\":{}',
                "desktop_frame.guest_trace_evidence",
            ),
            (
                contract.DESKTOP_HOT_SWAP_HARNESS,
                ".map(|frame| frame.guest_trace)",
                ".map(|frame| frame.trace)",
                "desktop_hot_swap.generation_oracle",
            ),
            (
                contract.DESKTOP_HOT_SWAP_HARNESS,
                "frame.guest_trace != expected_guest_trace",
                "frame.trace != expected_guest_trace",
                "desktop_hot_swap.all_history_oracle",
            ),
        )
        for path, old, new, field in mutations:
            with self.subTest(field=field):
                failures, _ = self.run_with(path, old, new)
                self.assertTrue(any(failure.field == field for failure in failures))

        desktop = self.sources[contract.DESKTOP]
        capture_start = desktop.index("        let guest_trace = frame_evidence")
        overlay_start = desktop.index("        play_error_toasts.append_to_buffers(")
        overlay_end = desktop.index("        gfx.gfx_submit_u8", overlay_start)
        capture = desktop[capture_start:overlay_start]
        overlays = copy.deepcopy(self.sources)
        overlays[contract.DESKTOP] = (
            desktop[:capture_start]
            + desktop[overlay_start:overlay_end]
            + capture
            + desktop[overlay_end:]
        )
        failures, _ = contract.check(overlays=overlays)
        self.assertTrue(any(
            failure.field == "desktop_frame.guest_trace_order"
            for failure in failures
        ))

    def test_render_parity_rejects_fixed_current_trace_oracles(self):
        mutations = (
            (
                contract.RENDER_PARITY_MANIFEST,
                '  "trace_fixture": "samples/render_parity/trace.stasis",',
                '  "trace_fixture": "samples/render_parity/trace.stasis",\n'
                '  "command_trace": 1853793133,',
                "render_parity.fixed_numeric_trace",
            ),
            (
                contract.RENDER_PARITY_MANIFEST,
                '  "trace_fixture": "samples/render_parity/trace.stasis",',
                (
                    '  "trace_fixture": "samples/render_parity/trace.stasis",\n'
                    + '  "'
                    + "workshop_"
                    + 'command_trace": 3533510058,'
                ),
                "render_parity.fixed_numeric_trace",
            ),
            (
                contract.COMPILER_AOT,
                "expected_result: ParityExpectedResult::Nonzero,",
                "expected_result: ParityExpectedResult::Exact(1_853_793_133),",
                "render_parity.compiler_semantic_trace",
            ),
        )
        for path, current, stale, field in mutations:
            with self.subTest(path=path):
                failures, _ = self.run_with(path, current, stale)
                failure = next(failure for failure in failures if failure.field == field)
                self.assertEqual("runtime/stasis_render_contract.h", failure.producer)
                self.assertEqual(path.as_posix(), failure.consumer)

    def test_desktop_manifest_fixture_requires_canonical_gfx_import(self):
        canonical = 'import "../../../src/stdlib/internal/gfx_cmd.stasis";'
        mutations = (
            "",
            'import "../../../src/stdlib/internal/host_frame.stasis";',
        )
        for replacement in mutations:
            with self.subTest(replacement=replacement):
                failures, _ = self.run_with(
                    contract.DESKTOP_MANIFEST_FIXTURE, canonical, replacement
                )
                failure = next(
                    failure for failure in failures
                    if failure.field == "gfx_cmd.import"
                )
                self.assertEqual("src/stdlib/internal/gfx_cmd.stasis", failure.producer)
                self.assertEqual(
                    "tests/stasis/seams/desktop_manifest_assets_probe.stasis",
                    failure.consumer,
                )

    def test_desktop_manifest_harness_requires_canonical_capacities(self):
        mutations = (
            ("vec![0; STASIS_RENDER_I32_COUNT]", "vec![0; 34608]", "gfx_cmd_i32"),
            (
                "vec![0.0; STASIS_RENDER_F32_COUNT]",
                "vec![0.0; 125060]",
                "gfx_cmd_f32",
            ),
            ("vec![0; STASIS_RENDER_U8_COUNT]", "vec![0; 65536]", "gfx_cmd_u8"),
        )
        for canonical, literal, lane in mutations:
            with self.subTest(lane=lane):
                failures, _ = self.run_with(
                    contract.DESKTOP_MANIFEST_HARNESS, canonical, literal
                )
                failure = next(
                    failure for failure in failures
                    if failure.field == f"{lane}.host_capacity"
                )
                self.assertEqual("crates/stasis_dynload/src/lib.rs", failure.producer)
                self.assertEqual(
                    "apps/stasis/tests/desktop_manifest_assets_seam.rs",
                    failure.consumer,
                )

    def test_it012_rejects_fixed_trace_oracles(self):
        mutations = (
            (
                contract.GENERATED_MOBILE_AOT_C,
                "#include <string.h>\n",
                "#include <string.h>\n\n#define IT012_EXPECTED_TRACE 2880741754u\n",
            ),
            (
                contract.GENERATED_MOBILE_AOT_RUST,
                'const GFX_CMD: &str = include_str!("../../../src/stdlib/internal/gfx_cmd.stasis");',
                'const GFX_CMD: &str = include_str!("../../../src/stdlib/internal/gfx_cmd.stasis");\nconst EXPECTED_TRACE: u32 = 2_880_741_754;',
            ),
        )
        for path, old, new in mutations:
            with self.subTest(path=path):
                failures, _ = self.run_with(path, old, new)
                failure = next(
                    failure for failure in failures
                    if failure.field == "it012.fixed_trace_oracle"
                )
                self.assertEqual("runtime/stasis_render_contract.h", failure.producer)
                self.assertEqual(path.as_posix(), failure.consumer)

    def test_current_desktop_seams_reject_fixed_trace_oracles(self):
        mutations = (
            (
                contract.DESKTOP_INPUT_FRAME_HARNESS,
                "use serde_json::json;",
                "use serde_json::json;\nconst INPUT_TRACE: i32 = 1845463013;",
            ),
            (
                contract.DESKTOP_DISPLAY_METRICS_HARNESS,
                "use serde_json::json;",
                "use serde_json::json;\nconst ODD_TRACE: i32 = -1172930515;",
            ),
            (
                contract.DESKTOP_MANIFEST_HARNESS,
                "use serde_json::json;",
                "use serde_json::json;\nconst RENDER_TRACE: i32 = 626372452;",
            ),
        )
        for path, old, new in mutations:
            with self.subTest(path=path):
                failures, _ = self.run_with(path, old, new)
                failure = next(
                    failure for failure in failures
                    if failure.field == "current_render_trace.fixed_numeric_oracle"
                )
                self.assertEqual("runtime/stasis_render_contract.h", failure.producer)
                self.assertEqual(path.as_posix(), failure.consumer)

    def test_it015_rejects_fixed_trace_oracle_and_requires_semantic_comparison(self):
        mutations = (
            (
                contract.MOBILE_PACKAGED_ASSETS_HARNESS,
                "use serde_json::json;",
                "use serde_json::json;\nconst EXPECTED_RENDER_TRACE: u32 = 158_004_337;",
                "it015.render_trace.fixed_numeric_oracle",
            ),
            (
                contract.MOBILE_PACKAGED_ASSETS_HARNESS,
                """    assert_ne!(
        trace, 0,
        "packaged asset render trace must accept the semantically validated current frame"
    );""",
                "    let _ = trace;",
                "it015.render_trace.nonzero",
            ),
            (
                contract.MOBILE_PACKAGED_ASSETS_NATIVE,
                "#include <math.h>",
                "#include <math.h>\n#define IT015_EXPECTED_TRACE 158004337u",
                "it015.render_trace.fixed_numeric_oracle",
            ),
            (
                contract.MOBILE_PACKAGED_ASSETS_NATIVE,
                "CHECK(actual_trace == expected_trace);",
                "CHECK(actual_trace != expected_trace);",
                "it015.semantic_oracle.comparison",
            ),
        )
        for path, old, new, field in mutations:
            with self.subTest(path=path, field=field):
                failures, _ = self.run_with(path, old, new)
                failure = next(failure for failure in failures if failure.field == field)
                self.assertEqual("runtime/stasis_render_contract.h", failure.producer)
                self.assertEqual(path.as_posix(), failure.consumer)

    def test_it012_semantic_oracle_requires_current_abi_and_trace(self):
        mutations = (
            (
                "STASIS_RENDER_VERSION;",
                "5;",
                "it012.semantic_oracle.version",
            ),
            (
                "stasis_render_trace(expected_i32, expected_f32, expected_u8);",
                "submitted_trace;",
                "it012.semantic_oracle.trace",
            ),
            (
                "CHECK(submitted_trace == expected_trace);",
                "CHECK(submitted_trace != expected_trace);",
                "it012.semantic_oracle.comparison",
            ),
        )
        for old, new, field in mutations:
            with self.subTest(field=field):
                failures, _ = self.run_with(
                    contract.GENERATED_MOBILE_AOT_C, old, new
                )
                failure = next(failure for failure in failures if failure.field == field)
                self.assertEqual("runtime/stasis_render_contract.h", failure.producer)
                self.assertEqual(
                    "runtime/tests/stasis_generated_mobile_integration.c",
                    failure.consumer,
                )

    def test_stasis_capacity_drift_names_both_sides(self):
        failures, _ = self.run_with(
            contract.GFX_CMD,
            "global gfx_cmd_i32: i32[67888];",
            "global gfx_cmd_i32: i32[35121];",
        )
        message = "\n".join(map(str, failures))
        self.assertIn("producer=runtime/stasis_render_contract.h", message)
        self.assertIn("consumer=src/stdlib/internal/gfx_cmd.stasis", message)
        self.assertIn("field=gfx_cmd_i32.length", message)
        self.assertIn("expected=67888 actual=35121", message)

    def test_java_version_drift_is_rejected(self):
        failures, _ = self.run_with(
            contract.JAVA_RENDERER,
            "static final int RENDER_VERSION = 7;",
            "static final int RENDER_VERSION = 5;",
        )
        self.assertTrue(any(failure.field == "STASIS_RENDER_VERSION" for failure in failures))

    def test_web_magic_drift_reports_contract_provenance(self):
        failures, _ = self.run_with(
            contract.WEB,
            "const GFX_CMD_MAGIC = 0x47584631;",
            "const GFX_CMD_MAGIC = 0x47584632;",
        )
        message = "\n".join(map(str, failures))
        self.assertIn("producer=runtime/stasis_render_contract.h", message)
        self.assertIn("consumer=runtime/web/game.js", message)
        self.assertIn("field=STASIS_RENDER_MAGIC", message)
        self.assertIn("expected=1196967473 actual=1196967474", message)

    def test_web_layout_drift_reports_field_and_values(self):
        failures, _ = self.run_with(
            contract.WEB,
            "const GFX_F_TEXT_BASE = 133252;",
            "const GFX_F_TEXT_BASE = 133253;",
        )
        failure = next(
            failure for failure in failures if failure.field == "STASIS_RENDER_F_TEXT_BASE"
        )
        self.assertEqual(133252, failure.expected)
        self.assertEqual(133253, failure.actual)

    def test_web_current_version_capacity_stride_and_offset_drift(self):
        mutations = (
            ("const GFX_CMD_VERSION = 7;",
             "const GFX_CMD_VERSION = 6;",
             "STASIS_RENDER_VERSION", 7, 6),
            ("const GFX_MAX_TEXT = 2048;", "const GFX_MAX_TEXT = 2047;",
             "STASIS_RENDER_MAX_TEXT", 2048, 2047),
            ("const GFX_SPRITE_STRIDE_F32 = 13;", "const GFX_SPRITE_STRIDE_F32 = 12;",
             "STASIS_RENDER_SPRITE_F32_STRIDE", 13, 12),
            ("const GFX_I_ORDER_BASE = 51232;", "const GFX_I_ORDER_BASE = 51233;",
             "STASIS_RENDER_I_ORDER_BASE", 51232, 51233),
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
            (contract.PACKAGE_PROVENANCE, "CURRENT_COMMAND_BUFFER_VERSION = 7",
             "CURRENT_COMMAND_BUFFER_VERSION = 6", "tools/verify_package_provenance.py"),
            (contract.TOOLCHAIN, "GFX_CMD_VERSION: i64 = 7",
             "GFX_CMD_VERSION: i64 = 6", "apps/stasis/src/toolchain_cli.rs"),
        )
        for path, old, new, consumer in mutations:
            failures, _ = self.run_with(path, old, new)
            failure = next(
                failure for failure in failures
                if failure.field == "STASIS_RENDER_VERSION"
                and failure.consumer == consumer
            )
            self.assertEqual(7, failure.expected)
            self.assertEqual(6, failure.actual)

    def test_rust_offset_drift_is_rejected(self):
        failures, _ = self.run_with(
            contract.DYNLOAD,
            "const STASIS_RENDER_ORDER_BASE: usize = 51_232;",
            "const STASIS_RENDER_ORDER_BASE: usize = 51_233;",
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
            "gfx_cmd_f32, 146564);",
            "gfx_cmd_f32, 126083);",
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
