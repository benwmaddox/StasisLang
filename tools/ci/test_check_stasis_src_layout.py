import tempfile
import unittest
from pathlib import Path

try:
    from .check_stasis_src_layout import discover_stasis_files
except ImportError:
    from check_stasis_src_layout import discover_stasis_files


class StasisSourceDiscoveryTests(unittest.TestCase):
    def test_excludes_vendored_sources_but_keeps_adjacent_application_sources(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            application = root / "sample" / "main.stasis"
            vendored = root / "sample" / "vendor" / "stasis" / "stdlib.stasis"
            application.parent.mkdir(parents=True)
            vendored.parent.mkdir(parents=True)
            application.write_text(
                'import "internal/host_frame.stasis";\n', encoding="utf-8"
            )
            vendored.write_text(
                'import "internal/host_frame.stasis";\n', encoding="utf-8"
            )

            discovered = discover_stasis_files([root])

            self.assertEqual(discovered, [application])


if __name__ == "__main__":
    unittest.main()
