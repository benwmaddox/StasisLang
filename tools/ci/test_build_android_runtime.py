import argparse
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "build_android_runtime", ROOT / "tools/build_android_runtime.py"
)
assert SPEC and SPEC.loader
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)


class AndroidRuntimeBuildTests(unittest.TestCase):
    def test_source_tree_hash_is_deterministic_and_ignores_build_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "src/runtime.c").write_text("one\n", encoding="ascii")
            baseline = BUILD.source_tree_hash(root)
            (root / "build").mkdir()
            (root / "build/output.a").write_bytes(b"ignored")
            self.assertEqual(BUILD.source_tree_hash(root), baseline)
            (root / "src/runtime.c").write_text("two\n", encoding="ascii")
            self.assertNotEqual(BUILD.source_tree_hash(root), baseline)

    def test_source_tree_hash_does_not_follow_external_or_dangling_links(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            root = base / "source"
            root.mkdir()
            (root / "runtime.c").write_text("source\n", encoding="ascii")
            external = base / "external.h"
            external.write_text("one\n", encoding="ascii")
            baseline = BUILD.source_tree_hash(root)
            try:
                (root / "external.h").symlink_to(external)
                (root / "dangling.h").symlink_to(base / "missing.h")
            except OSError as error:
                self.skipTest(f"symbolic links unavailable: {error}")
            self.assertEqual(BUILD.source_tree_hash(root), baseline)
            external.write_text("two\n", encoding="ascii")
            self.assertEqual(BUILD.source_tree_hash(root), baseline)

    def test_assemble_records_all_three_variants_and_file_hashes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runtime_source = root / "runtime"
            runtime_source.mkdir()
            (runtime_source / "stasis_mobile_runtime.h").write_text(
                "#define STASIS_MOBILE_RUNTIME_ABI_VERSION 2\n", encoding="ascii"
            )
            sdl3 = root / "SDL"
            (sdl3 / "include/SDL3").mkdir(parents=True)
            (sdl3 / "include/SDL3/SDL.h").write_text("/* SDL */\n", encoding="ascii")
            (sdl3 / "android-project/app/src/main/java/org/libsdl/app").mkdir(
                parents=True
            )
            (sdl3 / "android-project/app/src/main/java/org/libsdl/app/SDLActivity.java").write_text(
                "class SDLActivity {}\n", encoding="ascii"
            )
            (sdl3 / "LICENSE.txt").write_text("SDL license\n", encoding="ascii")
            sdl_image = root / "SDL_image"
            (sdl_image / "include/SDL3_image").mkdir(parents=True)
            (sdl_image / "include/SDL3_image/SDL_image.h").write_text(
                "/* SDL_image */\n", encoding="ascii"
            )
            (sdl_image / "LICENSE.txt").write_text("SDL_image license\n", encoding="ascii")
            build_root = root / "build-output"
            build_root.mkdir(parents=True)
            for variant in BUILD.VARIANTS:
                (build_root / f"libstasis_mobile_runtime_{variant}.a").write_bytes(
                    variant.encode("ascii")
                )
            for filename in ("libSDL3.a", "libSDL3_image.a", "libstasis_thorvg.a"):
                (build_root / filename).write_bytes(filename.encode("ascii"))
            output = root / "kit"
            args = argparse.Namespace(
                runtime_source=runtime_source,
                sdl3_source=sdl3,
                sdl3_image_source=sdl_image,
                out=output,
                release_id="nightly-20260929-747",
                source_commit="0123456789012345678901234567890123456789",
                build_fingerprint="11" * 32,
                cmake_version="3.22.1",
            )
            manifest = BUILD.assemble(args, build_root)
            self.assertEqual(set(manifest["runtime_variants"]), set(BUILD.VARIANTS))
            self.assertEqual(manifest["ndk_version"], BUILD.NDK_VERSION)
            self.assertGreaterEqual(len(manifest["files"]), 12)
            self.assertEqual(
                json.loads((output / "manifest.json").read_text(encoding="utf-8")),
                manifest,
            )

    def test_nightly_workflow_builds_and_accepts_prebuilt_runtime(self):
        workflow = (ROOT / ".github/workflows/nightly-release.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("android_runtime_support:", workflow)
        self.assertIn("android_prebuilt_acceptance:", workflow)
        self.assertIn("tools/build_android_runtime.py", workflow)
        self.assertIn("package-mobile (five variants)", workflow)
        self.assertIn('build-tools/35.0.0/apksigner" verify --verbose "${apk}"', workflow)
        self.assertIn(
            'network_audit=(--network-enabled --readelf "${readelf}")', workflow
        )
        self.assertIn(
            '--required-asset "" "${network_audit[@]}" "${apk}"', workflow
        )
        self.assertIn('"launcher_resources": "branding/android/res"', workflow)
        self.assertIn('mipmap-anydpi-v26/ic_launcher.xml', workflow)
        self.assertIn("does not claim the private consumer job is under 60 seconds", workflow)


if __name__ == "__main__":
    unittest.main()
