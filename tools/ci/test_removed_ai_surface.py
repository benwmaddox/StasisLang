"""Guard the removed AI frontends while retaining Workshop's manual entry points."""

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
WORKSHOP = ROOT / "mobile/android/app/src/workshop/java/com/stasislang/workshop"


class RemovedAiSurfaceTests(unittest.TestCase):
    def test_provider_modules_and_dependencies_are_absent(self):
        for relative in (
            "crates/stasis_ai",
            "mobile/android/codex_native",
            "mobile/android/build_codex_native.ps1",
            "apps/stasis/src/toolchain_cli/desktop_editor.rs",
            "apps/stasis/src/toolchain_cli/live_tui.rs",
            "apps/stasis/src/toolchain_cli/gauntlet.rs",
        ):
            with self.subTest(path=relative):
                self.assertFalse((ROOT / relative).exists())
        for relative in ("Cargo.toml", "Cargo.lock", "apps/stasis/Cargo.toml"):
            self.assertNotIn("stasis_ai", (ROOT / relative).read_text(encoding="utf-8"))

    def test_workshop_has_manual_controls_without_provider_or_ai_ui(self):
        activity = (WORKSHOP / "MainActivity.java").read_text(encoding="utf-8")
        for removed in (
            "nativeCodex", "nativeSharedAiContract", "runAiPatch", "api.openai.com",
            "AI Settings", "AI Work Queue", "Save + Attach to AI", "SpeechRecognizer",
        ):
            with self.subTest(removed=removed):
                self.assertNotIn(removed, activity)
        for retained in (
            "createProjectControls()", "applySelectedEdit()", "nativeRunTests(projectRootPath())",
            "captureFirstTestFailureDiagnostic(result)", "WorkshopProjectArchive.importProject",
            "WorkshopProjectArchive.exportProject", "restorePendingDraft()",
        ):
            with self.subTest(retained=retained):
                self.assertIn(retained, activity)
        for source in WORKSHOP.glob("*.java"):
            self.assertFalse(source.name.startswith(("WorkshopAi", "AndroidAi", "AiQueue", "WorkshopCodex")))


if __name__ == "__main__":
    unittest.main()
