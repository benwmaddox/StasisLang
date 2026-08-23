import pathlib
import tempfile
import unittest

from tools.ci.check_sdl3_migration import PINNED, validate


ROOT = pathlib.Path(__file__).resolve().parents[2]


class Sdl3MigrationContractTests(unittest.TestCase):
    def test_repository_uses_only_the_pinned_native_sdl3_family(self):
        self.assertEqual(validate(ROOT), [])

    def test_obsolete_sdl2_fallback_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for relative, markers in PINNED.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("\n".join(markers) + "\n", encoding="utf-8")
            runtime = root / "runtime/stasis_graphics.c"
            runtime.write_text(
                runtime.read_text(encoding="utf-8") + "sdl2-compat\n",
                encoding="utf-8",
            )
            self.assertTrue(any("obsolete" in error for error in validate(root)))

    def test_sdl3_boolean_return_using_sdl2_convention_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for relative, markers in PINNED.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("\n".join(markers) + "\n", encoding="utf-8")
            runtime = root / "runtime/stasis_graphics.c"
            runtime.write_text(
                runtime.read_text(encoding="utf-8")
                + "if (SDL_UpdateTexture(texture, NULL, pixels, pitch) != 0) {}\n",
                encoding="utf-8",
            )
            self.assertTrue(
                any("boolean return" in error for error in validate(root))
            )

    def test_obsolete_ios_startup_wrapper_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for relative, markers in PINNED.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("\n".join(markers) + "\n", encoding="utf-8")
            ios_main = root / "mobile/shells/ios/StasisMobile/main.m"
            ios_main.write_text(
                ios_main.read_text(encoding="utf-8")
                + "SDL_UIKitRunApp(argc, argv, SDL_main);\n",
                encoding="utf-8",
            )
            self.assertTrue(
                any("obsolete SDL2 startup wrapper" in error for error in validate(root))
            )

    def test_duplicate_ios_main_wrapper_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for relative, markers in PINNED.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("\n".join(markers) + "\n", encoding="utf-8")
            ios_main = root / "mobile/shells/ios/StasisMobile/main.m"
            ios_main.write_text(
                ios_main.read_text(encoding="utf-8") + "int main(void) { return 0; }\n",
                encoding="utf-8",
            )
            self.assertTrue(
                any("SDL3 must own the iOS main wrapper" in error for error in validate(root))
            )

    def test_ios_network_presenter_is_separate_and_compiled_by_xcode(self):
        ios_main = ROOT / "mobile/shells/ios/StasisMobile/main.m"
        network_source = ROOT / "mobile/shells/ios/StasisMobile/stasis_ios_network.m"
        project = ROOT / "mobile/shells/ios/StasisMobile.xcodeproj/project.pbxproj"
        self.assertEqual(ios_main.read_text(encoding="utf-8").strip(), "#include <SDL3/SDL_main.h>")
        network_text = network_source.read_text(encoding="utf-8")
        self.assertIn("stasis_mobile_network_present_join_url", network_text)
        self.assertNotIn("stasis_mobile_network_present_join_url", ios_main.read_text(encoding="utf-8"))
        project_text = project.read_text(encoding="utf-8")
        self.assertIn("stasis_ios_network.m in Sources", project_text)
        self.assertIn("path = StasisMobile/stasis_ios_network.m", project_text)

    def test_ios_ci_dependency_drift_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for relative, markers in PINNED.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("\n".join(markers) + "\n", encoding="utf-8")
            driver = root / "tools/ci/build_ios_package.sh"
            driver.write_text(
                driver.read_text(encoding="utf-8").replace(
                    "SDL3-3.4.10.dmg", "SDL3-rolling.dmg"
                ),
                encoding="utf-8",
            )
            self.assertTrue(
                any("SDL3-3.4.10.dmg" in error for error in validate(root))
            )


if __name__ == "__main__":
    unittest.main()
