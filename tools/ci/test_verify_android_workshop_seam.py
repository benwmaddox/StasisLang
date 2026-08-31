import json
import tempfile
import unittest
from pathlib import Path

try:
    from .verify_android_workshop_seam import SeamError, _read_json, verify_log
except ImportError:
    from verify_android_workshop_seam import SeamError, _read_json, verify_log


MANIFEST = {"state_checksum": 2500, "render_contract_version": 6}
GOOD = """CompileReady: backend=cranelift-jit reload=InitialCompile status=0 functions=7 compile_us=12 manifest=x
Stasis Workshop IT-025: {"schema":"stasis.workshop_seam.v1","test_id":"IT-025","event":"frame","jni_version":65542,"rust_bridge_version":"0.1.0","render_version":6,"state_checksum":2500,"command_trace":919191,"frame_token":1,"fallback":0,"stub":0}
Stasis Workshop IT-025 GLES: {"schema":"stasis.workshop_seam.v1","test_id":"IT-025","event":"present","count":1,"frame_token":1}
Stasis Workshop IT-025: {"schema":"stasis.workshop_seam.v1","test_id":"IT-025","event":"frame","jni_version":65542,"rust_bridge_version":"0.1.0","render_version":6,"state_checksum":2500,"command_trace":424242,"frame_token":50,"fallback":0,"stub":0}
RenderAcceptanceFrame: count=1 frame_token=1
Stasis Workshop IT-026: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"buffer_abi","status":"passed","descriptor":{"lanes":[{"lane":"i32","bytes":140480,"alignment":4},{"lane":"f32","bytes":504336,"alignment":4},{"lane":"u8","bytes":65536,"alignment":1}]},"valid_guards_intact":true,"all_invalid_unchanged":true,"valid_calls":1,"invalid_calls":18}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"short_i32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"i32","reason":"capacity","expected":140480,"actual":140479}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"short_f32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"f32","reason":"capacity","expected":504336,"actual":504335}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"short_u8","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"u8","reason":"capacity","expected":65536,"actual":65535}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"oversized_i32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"i32","reason":"capacity","expected":140480,"actual":140481}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"oversized_f32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"f32","reason":"capacity","expected":504336,"actual":504337}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"oversized_u8","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"u8","reason":"capacity","expected":65536,"actual":65537}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"swapped_i32_f32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"i32","reason":"capacity","expected":140480,"actual":504336}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"wrong_order_i32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"i32","reason":"byte_order","expected":"native","actual":"non_native"}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"wrong_order_f32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"f32","reason":"byte_order","expected":"native","actual":"non_native"}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"wrong_order_u8","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"u8","reason":"byte_order","expected":"native","actual":"non_native"}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"heap_i32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"i32","reason":"not_direct","expected":140480,"actual":-1}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"heap_f32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"f32","reason":"not_direct","expected":504336,"actual":-1}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"heap_u8","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"u8","reason":"not_direct","expected":65536,"actual":-1}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"null_i32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"i32","reason":"null_buffer","expected":140480,"actual":-1}}
	Stasis Workshop IT-026 case: {"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"case","name":"null_f32","unchanged":true,"error":{"schema":"stasis.workshop_jni_frame_abi.v1","test_id":"IT-026","event":"error","lane":"f32","reason":"null_buffer","expected":504336,"actual":-1}}
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
CompileError: src/main.stasis: cannot resolve call 'IT028_missing_target'|diagnostic_file=src/main.stasis|diagnostic_line=40|diagnostic_column=31|diagnostic_end_line=42|diagnostic_end_column=2|diagnostic_symbol=on_code_swap|diagnostic_message=cannot%20resolve%20call%20%27IT028_missing_target%27
Stasis Workshop IT-028 GLES: {"schema":"stasis.workshop_hot_edit.v1","test_id":"IT-028","event":"present","frame_token":83,"trace":115,"rect_count":2,"order_count":11,"marker":{"active":true,"x":176.0,"y":48.0,"w":24.0,"h":24.0,"r":0.2,"g":0.9,"b":0.95,"a":1.0}}
Stasis Workshop IT-028 case: {"schema":"stasis.workshop_hot_edit.v1","test_id":"IT-028","event":"case","status":"passed","phase":"post_invalid","sequence":3,"runtime":{"status":"RuntimeStateReady","generation":2,"source_fingerprint":"2222222222222222"},"guest":{"tick_revision":2,"render_revision":2,"state_counter":3},"render":{"trace":115,"frame_token":83,"rect_count":2,"marker":{"active":true,"x":176.0,"y":48.0,"w":24.0,"h":24.0,"r":0.2,"g":0.9,"b":0.95,"a":1.0}},"gles_presented":true,"gles_frame_token":83,"java_only":false,"fallback":0,"stub":0}
Stasis Workshop IT-028: {"schema":"stasis.workshop_hot_edit.v1","test_id":"IT-028","event":"hot_edit","status":"passed","ordered":true,"unique":true,"atomic":true,"hook_source_line":40,"invalid_compile":{"ok":false,"kind":"compile_error","diagnostic":{"file":"src/main.stasis","line":40,"column":31,"end_line":42,"end_column":2,"symbol":"on_code_swap","message":"cannot resolve call 'IT028_missing_target'"}},"restore_receipt":{"status":"NoChange","compile":"CompileReady: backend=cranelift-jit reload=NoChange status=0 functions=10 compile_us=12 manifest=build/native_compile_manifest.txt"},"cleanup_receipt":{"status":"Restored","compile":"CompileReady: backend=cranelift-jit reload=FastReload status=0 functions=10 compile_us=13 manifest=build/native_compile_manifest.txt","frame":{"status":"passed","runtime":{"generation":3,"source_fingerprint":"1111111111111111"},"render":{"marker":{"active":false}},"java_only":false,"fallback":0,"stub":0}}}
Stasis Workshop IT-031: {"schema":"stasis.workshop_diagnostic_seam.v1","test_id":"IT-031","event":"diagnostic_seam","status":"passed","ordered":true,"cases":[{"name":"parse","equal":true,"native":{"schema":"stasis.native_diagnostic.v1","version":1,"stage":"parse","code":"stasis.parse","context":{"file":"src/main.stasis"},"detail":"parse detail","causes":["parse phase","parse detail"]},"ui":{"schema":"stasis.native_diagnostic.v1","version":1,"stage":"parse","code":"stasis.parse","context":{"file":"src/main.stasis"},"detail":"parse detail","causes":["parse phase","parse detail"]}},{"name":"extern_resolution","equal":true,"native":{"schema":"stasis.native_diagnostic.v1","version":1,"stage":"extern_resolution","code":"stasis.unresolvedExtern","context":{"file":"src/main.stasis","symbol":"IT031_missing_extern"},"detail":"extern detail","causes":["extern_resolution phase","extern detail"]},"ui":{"schema":"stasis.native_diagnostic.v1","version":1,"stage":"extern_resolution","code":"stasis.unresolvedExtern","context":{"file":"src/main.stasis","symbol":"IT031_missing_extern"},"detail":"extern detail","causes":["extern_resolution phase","extern detail"]}},{"name":"runtime_entry","equal":true,"native":{"schema":"stasis.native_diagnostic.v1","version":1,"stage":"runtime_entry","code":"stasis.runtimeEntry","context":{"symbol":"tick"},"detail":"runtime detail","causes":["runtime_entry phase","runtime detail"]},"ui":{"schema":"stasis.native_diagnostic.v1","version":1,"stage":"runtime_entry","code":"stasis.runtimeEntry","context":{"symbol":"tick"},"detail":"runtime detail","causes":["runtime_entry phase","runtime detail"]}},{"name":"render_schema","equal":true,"native":{"schema":"stasis.native_diagnostic.v1","version":1,"stage":"render_schema","code":"stasis.renderSchema","context":{"symbol":"render"},"detail":"render detail","causes":["render_schema phase","render detail"]},"ui":{"schema":"stasis.native_diagnostic.v1","version":1,"stage":"render_schema","code":"stasis.renderSchema","context":{"symbol":"render"},"detail":"render detail","causes":["render_schema phase","render detail"]}},{"name":"missing_resource","equal":true,"native":{"schema":"stasis.native_diagnostic.v1","version":1,"stage":"resource","code":"stasis.missingResource","context":{"resource":"assets/IT031_missing.svg"},"detail":"resource detail","causes":["resource phase","resource detail"]},"ui":{"schema":"stasis.native_diagnostic.v1","version":1,"stage":"resource","code":"stasis.missingResource","context":{"resource":"assets/IT031_missing.svg"},"detail":"resource detail","causes":["resource phase","resource detail"]}}],"cleanup_receipt":{"status":"Restored","compile":"CompileReady: status=0","frame":"passed","source_fingerprint":"1111111111111111","baseline_source_fingerprint":"1111111111111111","generation":3,"baseline_generation":3,"ui":{"blocking_error_visible":false,"status_healthy":true,"compile_ready":true,"compile_attempted":true,"game_runtime_active":true,"displayed_status":"Game updated - hot swapped"}}}
Stasis Workshop IT-025: {"schema":"stasis.workshop_seam.v1","test_id":"IT-025","event":"frame","jni_version":65542,"rust_bridge_version":"0.1.0","render_version":6,"state_checksum":2500,"command_trace":3533510058,"frame_token":76,"fallback":0,"stub":0}
Stasis Workshop IT-025: {"schema":"stasis.workshop_seam.v1","test_id":"IT-025","event":"frame","jni_version":65542,"rust_bridge_version":"0.1.0","render_version":6,"state_checksum":2500,"command_trace":3533510058,"frame_token":77,"fallback":0,"stub":0}
RenderAcceptanceFrame: count=30 frame_token=77
Stasis Workshop IT-025 GLES: {"schema":"stasis.workshop_seam.v1","test_id":"IT-025","event":"present","count":30,"frame_token":77}
"""
# Keep the fixture's IT-031 case evidence in separate bounded log records, as
# the Android logcat line limit cannot carry five duplicated full cases.
_full_it031_line = next(line for line in GOOD.splitlines()
                        if line.startswith("Stasis Workshop IT-031: "))
_full_it031_marker = json.loads(_full_it031_line.split(": ", 1)[1])
_it031_cases = _full_it031_marker.pop("cases")
for _case in _it031_cases:
    _case["test_id"] = "IT-031"
_it031_cases[0]["location"] = {
    "expected": {"line": 3, "column": 1, "end_line": 4, "end_column": 1},
    "actual": {"line": 3, "column": 1, "end_line": 4, "end_column": 1}}
_full_it031_marker["case_count"] = len(_it031_cases)
_full_it031_marker["case_names"] = [case["name"] for case in _it031_cases]
_it031_case_lines = "\n".join(
    "Stasis Workshop IT-031 case: " + json.dumps(case, separators=(",", ":"))
    for case in _it031_cases)
GOOD = GOOD.replace(
    _full_it031_line,
    _it031_case_lines + "\nStasis Workshop IT-031: "
    + json.dumps(_full_it031_marker, separators=(",", ":")))


def _it031_log(marker: dict) -> str:
    cases = marker.get("cases", [])
    cases = [dict(case, test_id="IT-031") for case in cases]
    compact = {key: value for key, value in marker.items() if key != "cases"}
    compact["case_count"] = len(cases)
    compact["case_names"] = [case.get("name") for case in cases]
    case_lines = "\n".join(
        "Stasis Workshop IT-031 case: " + json.dumps(case, separators=(",", ":"))
        for case in cases)
    prefix = GOOD.split("Stasis Workshop IT-031 case:", 1)[0]
    lines = GOOD.splitlines()
    summary_index = next(
        index
        for index, line in enumerate(lines)
        if line.startswith("Stasis Workshop IT-031: ")
    )
    suffix = "\n".join(lines[summary_index + 1:])
    return prefix + case_lines + "\nStasis Workshop IT-031: " \
        + json.dumps(compact, separators=(",", ":")) + "\n" + suffix


# A successful cleanup publication advances the live runtime generation while
# restoring the original source fingerprint.
GOOD = GOOD.replace('"generation":3,"baseline_generation":3',
                    '"generation":4,"baseline_generation":3')
GOOD = GOOD.replace('"context":{"file":"src/main.stasis"},"detail":"parse detail"',
                    '"context":{"file":"src/main.stasis","symbol":"on_code_swap"},'
                    '"detail":"parse detail"')
GOOD = GOOD.replace('"causes":["parse phase","parse detail"]}},',
                    '"causes":["parse phase","parse detail"]},'
                    '"location":{"expected":{"line":3,"column":1,"end_line":4,"end_column":1},'
                    '"actual":{"line":3,"column":1,"end_line":4,"end_column":1}}},', 1)

for _name, _detail in (("parse", "parse detail"), ("extern_resolution", "extern detail"),
                       ("runtime_entry", "runtime detail"), ("render_schema", "render detail"),
                       ("missing_resource", "resource detail")):
    GOOD = GOOD.replace(
        '{"name":"' + _name + '","equal":true,',
        '{"name":"' + _name + '","equal":true,"displayed_text":"' + _detail + '",',
        1)


class WorkshopSeamTests(unittest.TestCase):
    def test_accepts_ordered_it031_native_ui_diagnostics(self):
        cases = []
        for name, stage, code in [
                ("parse", "parse", "stasis.parse"),
                ("extern_resolution", "extern_resolution", "stasis.unresolvedExtern"),
                ("runtime_entry", "runtime_entry", "stasis.runtimeEntry"),
                ("render_schema", "render_schema", "stasis.renderSchema"),
                ("missing_resource", "resource", "stasis.missingResource")]:
            diagnostic = {"schema": "stasis.native_diagnostic.v1", "version": 1,
                          "stage": stage, "code": code, "context": {},
                          "detail": name + " detail",
                          "causes": [stage + " phase", name + " detail"]}
            if name in {"parse", "extern_resolution"}:
                diagnostic["context"]["file"] = "src/main.stasis"
            if name == "parse":
                diagnostic["context"]["symbol"] = "on_code_swap"
            if name == "extern_resolution":
                diagnostic["context"]["symbol"] = "IT031_missing_extern"
            if name == "missing_resource":
                diagnostic["context"]["resource"] = "assets/IT031_missing.svg"
            if name == "runtime_entry":
                diagnostic["context"]["symbol"] = "tick"
            if name == "render_schema":
                diagnostic["context"]["symbol"] = "render"
            case = {"name": name, "native": diagnostic, "ui": diagnostic,
                    "displayed_text": diagnostic["detail"], "equal": True}
            if name == "parse":
                case["location"] = {
                    "expected": {"line": 3, "column": 1, "end_line": 4, "end_column": 1},
                    "actual": {"line": 3, "column": 1, "end_line": 4, "end_column": 1}}
            cases.append(case)
        marker = {"schema": "stasis.workshop_diagnostic_seam.v1", "test_id": "IT-031",
                  "event": "diagnostic_seam", "status": "passed", "ordered": True,
                  "cases": cases, "cleanup_receipt": {"status": "Restored", "compile": "CompileReady: status=0", "frame": "passed", "source_fingerprint": "1111111111111111", "baseline_source_fingerprint": "1111111111111111", "generation": 4, "baseline_generation": 3, "ui": {"blocking_error_visible": False, "status_healthy": True, "compile_ready": True, "compile_attempted": True, "game_runtime_active": True, "displayed_status": "Game updated - hot swapped"}}}
        result = verify_log(_it031_log(marker), MANIFEST)
        self.assertEqual(result["it031"]["test_id"], "IT-031")

    def test_rejects_it031_changed_java_detail(self):
        cases = []
        for name, stage, code in [
                ("parse", "parse", "stasis.parse"),
                ("extern_resolution", "extern_resolution", "stasis.unresolvedExtern"),
                ("runtime_entry", "runtime_entry", "stasis.runtimeEntry"),
                ("render_schema", "render_schema", "stasis.renderSchema"),
                ("missing_resource", "resource", "stasis.missingResource")]:
            diagnostic = {"schema": "stasis.native_diagnostic.v1", "version": 1,
                          "stage": stage, "code": code, "context": {},
                          "detail": name, "causes": [stage + " phase", name]}
            if name in {"parse", "extern_resolution"}:
                diagnostic["context"]["file"] = "src/main.stasis"
            if name == "parse":
                diagnostic["context"]["symbol"] = "on_code_swap"
            if name == "extern_resolution":
                diagnostic["context"]["symbol"] = "IT031_missing_extern"
            if name == "missing_resource":
                diagnostic["context"]["resource"] = "assets/IT031_missing.svg"
            if name == "runtime_entry":
                diagnostic["context"]["symbol"] = "tick"
            if name == "render_schema":
                diagnostic["context"]["symbol"] = "render"
            ui = dict(diagnostic)
            ui["detail"] = "changed"
            cases.append({"name": name, "native": diagnostic, "ui": ui,
                          "displayed_text": diagnostic["detail"], "equal": True})
        marker = {"schema": "stasis.workshop_diagnostic_seam.v1", "test_id": "IT-031",
                  "event": "diagnostic_seam", "status": "passed", "ordered": True,
                  "cases": cases, "cleanup_receipt": {"status": "Restored", "compile": "CompileReady: status=0", "frame": "passed", "source_fingerprint": "1111111111111111", "baseline_source_fingerprint": "1111111111111111", "generation": 4, "baseline_generation": 3, "ui": {"blocking_error_visible": False, "status_healthy": True, "compile_ready": True, "compile_attempted": True, "game_runtime_active": True, "displayed_status": "Game updated - hot swapped"}}}
        with self.assertRaisesRegex(SeamError, "changed between native and UI"):
            verify_log(_it031_log(marker), MANIFEST)

    def test_rejects_missing_it031_marker(self):
        summary = next(
            line for line in GOOD.splitlines() if line.startswith("Stasis Workshop IT-031: ")
        )
        with self.assertRaisesRegex(SeamError, "IT-031"):
            verify_log(GOOD.replace(summary + "\n", "", 1), MANIFEST)

    def test_rejects_missing_it031_case_marker(self):
        case = next(line for line in GOOD.splitlines() if "IT-031 case" in line)
        with self.assertRaisesRegex(SeamError, "exactly 5 IT-031 cases"):
            verify_log(GOOD.replace(case + "\n", "", 1), MANIFEST)

    def test_rejects_it031_case_order_change(self):
        cases = [line for line in GOOD.splitlines() if "IT-031 case" in line]
        swapped = GOOD.replace(cases[0] + "\n" + cases[1],
                               cases[1] + "\n" + cases[0], 1)
        with self.assertRaisesRegex(SeamError, "ordered native cases"):
            verify_log(swapped, MANIFEST)

    def test_rejects_truncated_it031_summary(self):
        summary = next(line for line in GOOD.splitlines()
                       if line.startswith("Stasis Workshop IT-031: "))
        with self.assertRaisesRegex(SeamError, "IT-031"):
            verify_log(GOOD.replace(summary, summary[:-1], 1), MANIFEST)

    def test_rejects_malformed_it031_case_record(self):
        case = next(line for line in GOOD.splitlines() if "IT-031 case" in line)
        malformed = case.rsplit("}", 1)[0]
        with self.assertRaisesRegex(SeamError, "invalid IT-031 case JSON|exactly 5 IT-031 cases"):
            verify_log(GOOD.replace(case, malformed, 1), MANIFEST)

    def test_rejects_reversed_causes_and_generic_fallback(self):
        reversed_causes = GOOD.replace(
            '"causes":["parse phase","parse detail"]',
            '"causes":["parse detail","parse phase"]')
        with self.assertRaisesRegex(SeamError, "cause"):
            verify_log(reversed_causes, MANIFEST)
        generic_detail = GOOD.replace('"detail":"render detail"',
                                      '"detail":"native preview frame failed"', 1)
        with self.assertRaisesRegex(SeamError, "forbidden"):
            verify_log(generic_detail, MANIFEST)

    def test_rejects_wrong_it031_case_order_context_and_cleanup(self):
        wrong_code = GOOD.replace('"code":"stasis.renderSchema"',
                                  '"code":"stasis.parse"')
        with self.assertRaisesRegex(SeamError, "stage, code"):
            verify_log(wrong_code, MANIFEST)
        wrong_context = GOOD.replace('"symbol":"tick"', '"symbol":"main"')
        with self.assertRaisesRegex(SeamError, "tick symbol"):
            verify_log(wrong_context, MANIFEST)
        cleanup_failure = GOOD.replace(
            '"cleanup_receipt":{"status":"Restored","compile":"CompileReady: status=0","frame":"passed","source_fingerprint":"1111111111111111","baseline_source_fingerprint":"1111111111111111","generation":4,"baseline_generation":3,"ui":{"blocking_error_visible":false,"status_healthy":true,"compile_ready":true,"compile_attempted":true,"game_runtime_active":true,"displayed_status":"Game updated - hot swapped"}}',
            '"cleanup_receipt":{"status":"Restored","compile":"CompileReady: status=1","frame":"failed","source_fingerprint":"1111111111111111","baseline_source_fingerprint":"1111111111111111","generation":4,"baseline_generation":3,"ui":{"blocking_error_visible":false,"status_healthy":true,"compile_ready":true,"compile_attempted":true,"game_runtime_active":true,"displayed_status":"Game updated - hot swapped"}}', 1)
        with self.assertRaisesRegex(SeamError, "cleanup"):
            verify_log(cleanup_failure, MANIFEST)

    def test_rejects_it031_cleanup_identity_mismatch(self):
        mismatch = GOOD.replace(
            '"baseline_source_fingerprint":"1111111111111111"',
            '"baseline_source_fingerprint":"2222222222222222"', 1)
        with self.assertRaisesRegex(SeamError, "cleanup"):
            verify_log(mismatch, MANIFEST)

    def test_rejects_it031_nonadvancing_cleanup_generation(self):
        unchanged = GOOD.replace(
            '"generation":4,"baseline_generation":3',
            '"generation":3,"baseline_generation":3', 1)
        with self.assertRaisesRegex(SeamError, "cleanup"):
            verify_log(unchanged, MANIFEST)

    def test_rejects_it031_cleanup_with_blocking_ui_status(self):
        blocking = GOOD.replace('"blocking_error_visible":false',
                                '"blocking_error_visible":true', 1)
        with self.assertRaisesRegex(SeamError, "cleanup"):
            verify_log(blocking, MANIFEST)

    def test_rejects_it031_cleanup_without_ui_recovery_receipt(self):
        missing_ui = GOOD.replace(
            ',"ui":{"blocking_error_visible":false,"status_healthy":true,"compile_ready":true,"compile_attempted":true,"game_runtime_active":true,"displayed_status":"Game updated - hot swapped"}',
            '', 1)
        with self.assertRaisesRegex(SeamError, "cleanup"):
            verify_log(missing_ui, MANIFEST)

    def test_rejects_it031_parse_provenance_loss(self):
        wrong_symbol = GOOD.replace(
            '"context":{"file":"src/main.stasis","symbol":"on_code_swap"},'
            '"detail":"parse detail"',
            '"context":{"file":"src/main.stasis","symbol":"first"},'
            '"detail":"parse detail"')
        with self.assertRaisesRegex(SeamError, "final-function span or symbol"):
            verify_log(wrong_symbol, MANIFEST)
        missing_span = GOOD.replace(
            '"location":{"expected":{"line":3,"column":1,"end_line":4,"end_column":1},'
            '"actual":{"line":3,"column":1,"end_line":4,"end_column":1}}',
            '"location":{}', 1)
        with self.assertRaisesRegex(SeamError, "final-function span or symbol"):
            verify_log(missing_span, MANIFEST)

    def test_native_compile_transport_preserves_full_diagnostic(self):
        native = (Path(__file__).resolve().parents[2]
                  / "mobile/android/app/src/main/cpp/stasis_mobile_smoke.c").read_text()
        start = native.index("Java_com_stasislang_workshop_MainActivity_nativeCompileProject")
        end = native.index("Java_com_stasislang_workshop_MainActivity_nativeSourceItems", start)
        compile_native = native[start:end]
        self.assertNotIn("char message[", compile_native)
        self.assertNotIn("call_rust_bridge_compile", compile_native)
        self.assertIn('bridge->compile_project(root, "src/main.stasis")', compile_native)
        self.assertIn("jstring result = (*env)->NewStringUTF(env, message);", compile_native)
        self.assertIn("bridge->free_string(message);", compile_native)

    def test_codex_bridge_missing_symbols_invalidate_partial_api(self):
        native = (Path(__file__).resolve().parents[2]
                  / "mobile/android/app/src/main/cpp/stasis_mobile_smoke.c").read_text()
        start = native.index("static CodexBridgeApi *load_codex_bridge_api(void)")
        end = native.index("static jstring call_codex_bridge", start)
        loader = native[start:end]
        required_failure = loader.index("if (codex_bridge_api.initialize == NULL")
        close = loader.index("dlclose(codex_bridge_api.handle);", required_failure)
        clear = loader.index(
            "memset(&codex_bridge_api, 0, sizeof(codex_bridge_api));", close)
        attempted = loader.index("codex_bridge_api.attempted = 1;", clear)
        unavailable = loader.index("return NULL;", attempted)

        self.assertLess(close, clear)
        self.assertLess(clear, attempted)
        self.assertLess(attempted, unavailable)
        self.assertIn(
            "return codex_bridge_api.handle == NULL ? NULL : &codex_bridge_api;",
            loader,
        )

    def test_native_diagnostic_envelope_has_a_full_string_transport_contract(self):
        bridge = (Path(__file__).resolve().parents[2]
                  / "crates/stasis_android_bridge/src/lib.rs").read_text()
        native = (Path(__file__).resolve().parents[2]
                  / "mobile/android/app/src/main/cpp/stasis_mobile_smoke.c").read_text()
        self.assertIn('"schema": "stasis.native_diagnostic.v1"', bridge)
        self.assertIn("diagnostic_envelope=", bridge)
        self.assertIn("NewStringUTF(env, message)", native)
        start = bridge.index("fn format_native_diagnostic")
        end = bridge.index("fn format_runtime_diagnostic", start)
        self.assertNotIn("native preview frame failed", bridge[start:end])

    def test_accepts_complete_single_frame_proof(self):
        result = verify_log(GOOD, MANIFEST)
        self.assertEqual(result["compile_functions"], 7)
        self.assertEqual(result["presented_frames"], 30)
        self.assertEqual(result["it028"]["test_id"], "IT-028")

    def test_count30_edge_uses_it031_boundary_after_count1_predecessor(self):
        presentations = [
            line
            for line in GOOD.splitlines()
            if line.startswith("Stasis Workshop IT-025 GLES:")
        ]
        self.assertEqual(2, len(presentations))
        self.assertIn('"count":1', presentations[0])
        self.assertIn('"count":30', presentations[1])
        self.assertNotIn('"count":29', GOOD)
        self.assertEqual(verify_log(GOOD, MANIFEST)["presented_frames"], 30)

    def test_rejects_wrong_guest_state(self):
        with self.assertRaisesRegex(SeamError, "state_checksum"):
            verify_log(GOOD.replace('"state_checksum":2500', '"state_checksum":2501'), MANIFEST)

    def test_accepts_any_positive_stable_current_trace(self):
        result = verify_log(GOOD.replace("3533510058", "919191"), MANIFEST)
        self.assertEqual(result["presented_frames"], 30)

    def test_accepts_deliberate_trace_changes_before_idle_proof_window(self):
        earlier_changes = GOOD.replace(
            '"command_trace":919191,"frame_token":1',
            '"command_trace":111111,"frame_token":1',
        ).replace(
            '"command_trace":424242,"frame_token":50',
            '"command_trace":222222,"frame_token":50',
        )
        result = verify_log(earlier_changes, MANIFEST)
        self.assertEqual(result["presented_frames"], 30)

    def test_rejects_zero_or_changed_trace_within_idle_proof_window(self):
        with self.assertRaisesRegex(SeamError, "positive"):
            verify_log(GOOD.replace('"command_trace":3533510058', '"command_trace":0'), MANIFEST)
        with self.assertRaisesRegex(SeamError, "changed"):
            verify_log(
                GOOD.replace(
                    '"command_trace":3533510058,"frame_token":77',
                    '"command_trace":919191,"frame_token":77',
                ),
                MANIFEST,
            )

    def test_accepts_later_positive_trace_change_after_stable_presentation(self):
        stable_marker = next(
            line
            for line in GOOD.splitlines()
            if line.startswith("Stasis Workshop IT-025:") and '"frame_token":77' in line
        )
        later_marker = stable_marker.replace(
            '"command_trace":3533510058,"frame_token":77',
            '"command_trace":919191,"frame_token":999',
        )
        result = verify_log(GOOD + "\n" + later_marker, MANIFEST)
        self.assertEqual(result["presented_frames"], 30)

    def test_requires_preceding_presentation(self):
        presentation_lines = [
            line for line in GOOD.splitlines() if line.startswith("Stasis Workshop IT-025 GLES:")
        ]
        without_predecessors = GOOD
        for line in presentation_lines[:-1]:
            without_predecessors = without_predecessors.replace(line + "\n", "", 1)
        with self.assertRaisesRegex(SeamError, "lacks a preceding presentation"):
            verify_log(without_predecessors, MANIFEST)

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
        summary = next(
            line for line in GOOD.splitlines() if line.startswith("Stasis Workshop IT-026: ")
        )
        with self.assertRaisesRegex(SeamError, "IT-026"):
            verify_log(GOOD.replace(summary + "\n", "", 1), MANIFEST)

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
            verify_log(GOOD.replace('"actual":140479', '"actual":140478'), MANIFEST)

    def test_rejects_duplicate_it026_case(self):
        short_i32 = next(line for line in GOOD.splitlines() if '"name":"short_i32"' in line)
        short_f32 = next(line for line in GOOD.splitlines() if '"name":"short_f32"' in line)
        with self.assertRaisesRegex(SeamError, "scenarios mismatch"):
            verify_log(GOOD.replace(short_f32, short_i32), MANIFEST)

    def test_rejects_duplicate_it026_descriptor_lane(self):
        with self.assertRaisesRegex(SeamError, "ordered and unique"):
            verify_log(GOOD.replace('"lane":"f32","bytes":504336',
                                    '"lane":"i32","bytes":504336', 1), MANIFEST)

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
        summary = next(
            line
            for line in GOOD.splitlines()
            if line.startswith("Stasis Workshop IT-027: ")
        )
        with self.assertRaisesRegex(SeamError, "IT-027"):
            verify_log(GOOD.replace(summary + "\n", "", 1), MANIFEST)

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

    def test_rejects_missing_it028_raw_compile_error(self):
        raw = next(line for line in GOOD.splitlines() if line.startswith("CompileError: "))
        with self.assertRaisesRegex(SeamError, "exactly one raw CompileError"):
            verify_log(GOOD.replace(raw + "\n", ""), MANIFEST)

    def test_rejects_extra_or_unrelated_it028_raw_compile_error(self):
        raw = next(line for line in GOOD.splitlines() if line.startswith("CompileError: "))
        summary = next(line for line in GOOD.splitlines()
                       if line.startswith("Stasis Workshop IT-028: "))
        with self.assertRaisesRegex(SeamError, "exactly one raw CompileError"):
            verify_log(GOOD.replace(summary, raw + "\n" + summary, 1), MANIFEST)
        with self.assertRaisesRegex(SeamError, "truncated|mismatched|out of order"):
            verify_log(GOOD.replace(raw, "CompileError: unrelated failure", 1), MANIFEST)

    def test_rejects_truncated_it028_raw_compile_error(self):
        raw = next(line for line in GOOD.splitlines() if line.startswith("CompileError: "))
        truncated = raw.split("|diagnostic_message=", 1)[0]
        with self.assertRaisesRegex(SeamError, "truncated|mismatched|out of order"):
            verify_log(GOOD.replace(raw, truncated, 1), MANIFEST)

    def test_rejects_embedded_or_prefixed_it028_raw_compile_error(self):
        raw = next(line for line in GOOD.splitlines() if line.startswith("CompileError: "))
        published = next(line for line in GOOD.splitlines()
                         if '"phase":"published"' in line)
        with self.assertRaisesRegex(SeamError, "truncated|mismatched|out of order"):
            verify_log(GOOD.replace(published + "\n" + raw, published + raw, 1), MANIFEST)
        for prefix in ("Not", "arbitrary prefix "):
            with self.subTest(prefix=prefix):
                with self.assertRaisesRegex(SeamError, "truncated|mismatched|out of order"):
                    verify_log(GOOD.replace(raw, prefix + raw, 1), MANIFEST)

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
            ('"column":31', '"column":32'),
            ('"end_line":42', '"end_line":41'),
            ('"end_column":2', '"end_column":3'),
            ('"file":"src/main.stasis"', '"file":"src/other.stasis"'),
            ('"symbol":"on_code_swap"', '"symbol":"other"'),
            ('"message":"cannot resolve call \'IT028_missing_target\'"',
             '"message":"other diagnostic"'),
        ]
        for before, after in replacements:
            with self.subTest(field=before):
                with self.assertRaisesRegex(SeamError, "diagnostic"):
                    verify_log(GOOD.replace(before, after, 1), MANIFEST)

    def test_rejects_stale_it028_unknown_call_wording(self):
        stale = GOOD.replace(
            "cannot resolve call 'IT028_missing_target'",
            "unknown call target 'IT028_missing_target'",
            1,
        )
        with self.assertRaisesRegex(SeamError, "diagnostic"):
            verify_log(stale, MANIFEST)

    def test_rejects_it028_cleanup_failure(self):
        with self.assertRaisesRegex(SeamError, "forbidden"):
            verify_log(GOOD + "\nIT-028 cleanup failed: StateError: unavailable\n", MANIFEST)

    def test_it028_ignores_later_it031_compile_errors_and_summary_text(self):
        later = GOOD.replace('"displayed_text":"resource detail"',
                             '"displayed_text":"CompileError: resource detail"', 1)
        later += "\nCompileError: src/main.stasis: later IT-031 resource failure\n"
        result = verify_log(later, MANIFEST)
        self.assertEqual(result["it028"]["test_id"], "IT-028")


if __name__ == "__main__":
    unittest.main()
