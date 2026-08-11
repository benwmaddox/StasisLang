import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from tools.ci import verify_android_native_library as verifier


HEADER = """ELF Header:
  Class:                             ELF64
  Type:                              DYN (Shared object file)
  Machine:                           AArch64
"""
DYNAMIC = """Dynamic section:
  (NEEDED) Shared library: [libSDL3.so]
  (NEEDED) Shared library: [libSDL3_image.so]
  (NEEDED) Shared library: [liblog.so]
  (NEEDED) Shared library: [libandroid.so]
  (NEEDED) Shared library: [libc.so]
"""


def symbol_table(*defined: str, undefined: tuple[str, ...] = ()) -> str:
    lines = ["Symbol table '.dynsym':"]
    for index, name in enumerate(undefined, 1):
        lines.append(f" {index}: 0000000000000000 0 FUNC GLOBAL DEFAULT UND {name}")
    offset = len(undefined) + 1
    for index, name in enumerate(defined, offset):
        lines.append(f" {index}: 0000000000001000 4 FUNC GLOBAL DEFAULT 12 {name}")
    return "\n".join(lines)


class AndroidNativeLibraryAuditTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.library = self.root / "libmain.so"
        self.library.write_bytes(b"elf")
        self.readelf = self.root / "llvm-readelf"
        self.readelf.write_text("fixture", encoding="utf-8")
        (self.root / "main_1.o").write_bytes(b"object")
        (self.root / "tick_2.o").write_bytes(b"object")
        (self.root / "published_aot_bindings.c").write_text("bindings", encoding="utf-8")
        (self.root / "published_aot_objects.cmake").write_text(
            "set(STASIS_PUBLISHED_AOT_OBJECTS\n"
            '  "${CMAKE_CURRENT_LIST_DIR}/main_1.o"\n'
            '  "${CMAKE_CURRENT_LIST_DIR}/tick_2.o"\n'
            ")\n",
            encoding="utf-8",
        )
        engine = {
            "functions": [
                {"function_id": 1, "symbol": "aot_fn_1"},
                {"function_id": 2, "symbol": "aot_fn_2"},
            ]
        }
        (self.root / "engine_bundle_manifest.json").write_text(
            json.dumps(engine), encoding="utf-8"
        )
        bundle = {
            "schema": "stasis.mobile_aot_bundle.v1",
            "target": "android-arm64",
            "engine_manifest": "engine_bundle_manifest.json",
            "bindings_source": "published_aot_bindings.c",
            "android_cmake_file": "published_aot_objects.cmake",
            "objects": [
                {"function": "main", "function_id": 1, "path": "main_1.o"},
                {"function": "tick", "function_id": 2, "path": "tick_2.o"},
            ],
        }
        self.bundle = self.root / "mobile_aot_bundle_manifest.json"
        self.bundle.write_text(json.dumps(bundle), encoding="utf-8")
        self.link_map = self.root / "libmain.map"
        self.link_map.write_text(
            "main_1.o\ntick_2.o\npublished_aot_bindings.c.o\n", encoding="utf-8"
        )

    def tearDown(self):
        self.temporary.cleanup()

    def readelf_output(self, _readelf, _library, *options):
        if options == ("-h",):
            return HEADER
        if options == ("-d",):
            return DYNAMIC
        if options == ("-Ws",):
            return symbol_table(
                *sorted(verifier.REQUIRED_DEFINED), "aot_fn_1", "aot_fn_2", undefined=("SDL_Log",)
            )
        self.fail(f"unexpected readelf options: {options}")

    @mock.patch.object(verifier, "_run_readelf")
    def test_audit_cross_checks_manifest_map_and_final_elf(self, run_readelf):
        run_readelf.side_effect = self.readelf_output
        evidence = verifier.audit(self.library, self.bundle, self.link_map, self.readelf)
        self.assertEqual(evidence["test_id"], "IT-016")
        self.assertEqual(evidence["generated_objects"], 2)
        self.assertEqual(evidence["unresolved_stasis_symbols"], [])

    @mock.patch.object(verifier, "_run_readelf")
    def test_missing_manifest_object_names_the_link_map_gap(self, run_readelf):
        run_readelf.side_effect = self.readelf_output
        self.link_map.write_text("main_1.o\npublished_aot_bindings.c.o\n", encoding="utf-8")
        with self.assertRaisesRegex(
            verifier.AuditError, r"link map is missing packaged AOT objects: \['tick_2.o'\]"
        ):
            verifier.audit(self.library, self.bundle, self.link_map, self.readelf)

    @mock.patch.object(verifier, "_run_readelf")
    def test_unresolved_generated_symbol_is_actionable(self, run_readelf):
        def output(_readelf, _library, *options):
            if options == ("-h",):
                return HEADER
            if options == ("-d",):
                return DYNAMIC
            return symbol_table(
                *sorted(verifier.REQUIRED_DEFINED), "aot_fn_1", undefined=("aot_fn_2",)
            )

        run_readelf.side_effect = output
        with self.assertRaisesRegex(
            verifier.AuditError, r"missing generated/mobile symbols: \['aot_fn_2'\]"
        ):
            verifier.audit(self.library, self.bundle, self.link_map, self.readelf)


if __name__ == "__main__":
    unittest.main()
