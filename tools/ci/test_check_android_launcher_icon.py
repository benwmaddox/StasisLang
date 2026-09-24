"""Focused contract tests for compiled Android launcher resources."""

import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

from tools.ci import check_android_launcher_icon as icon


class CompiledLauncherIconTest(unittest.TestCase):
    MANIFEST = ('E: application\n'
                '  A: android:icon(0x01010002)=@0x7f020000\n'
                '  A: android:roundIcon(0x0101052c)=@0x7f020000\n')

    @staticmethod
    def resources(densities=(*icon.DENSITIES, 'anydpi')):
        return ('spec resource 0x7f020000 com.example:mipmap/ic_launcher: flags=0x00000000\n'
                + ''.join(f'  config {name}:\n    resource 0x7f020000 t=0x03\n' for name in densities))

    def test_apk_rejects_missing_manifest_icon(self):
        with patch.object(icon, '_run', return_value='E: application\n'):
            with self.assertRaisesRegex(ValueError, 'android:icon'):
                icon._verify_apk(Path('game.apk'), 'aapt')

    def test_apk_rejects_missing_density(self):
        outputs = iter((self.MANIFEST, self.resources(icon.DENSITIES)))
        with patch.object(icon, '_run', side_effect=lambda _command: next(outputs)):
            with self.assertRaisesRegex(ValueError, 'anydpi'):
                icon._verify_apk(Path('game.apk'), 'aapt')

    def test_apk_rejects_nonadaptive_anydpi(self):
        outputs = iter((self.MANIFEST, self.resources(),
                        "application-icon-65534:'res/icon.xml'\n", 'E: selector\n'))
        with patch.object(icon, '_run', side_effect=lambda _command: next(outputs)):
            with self.assertRaisesRegex(ValueError, 'adaptive icon'):
                icon._verify_apk(Path('game.apk'), 'aapt')

    def test_aab_rejects_missing_density_and_missing_compiled_entry(self):
        manifest = '<manifest><application android:icon="@mipmap/ic_launcher" android:roundIcon="@mipmap/ic_launcher"></application></manifest>'
        def table(densities):
            return 'mipmap/ic_launcher\n' + ''.join(
                f'density: {density} - [FILE] res/mipmap-{density}/ic_launcher.png\n'
                for density in densities)
        with tempfile.TemporaryDirectory() as tmp:
            bundle = Path(tmp) / 'game.aab'
            with zipfile.ZipFile(bundle, 'w') as archive:
                archive.writestr('base/manifest/AndroidManifest.xml', 'fixture')
            with patch.object(icon, '_tool', return_value='java'), patch.object(
                icon, '_run', side_effect=[manifest, table(icon.AAB_DENSITIES[:-1])]
            ):
                with self.assertRaisesRegex(ValueError, 'missing densities'):
                    icon._verify_aab(bundle, __file__, '')
            adaptive = 'density: 65534 - [FILE] res/mipmap-anydpi-v26/ic_launcher.xml\n'
            complete = table(icon.AAB_DENSITIES[:-1]) + adaptive
            with patch.object(icon, '_tool', return_value='java'), patch.object(
                icon, '_run', side_effect=[manifest, complete]
            ):
                with self.assertRaisesRegex(ValueError, 'absent from bundle'):
                    icon._verify_aab(bundle, __file__, '')


if __name__ == '__main__':
    unittest.main()
