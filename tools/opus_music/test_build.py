import hashlib
from pathlib import Path
import tempfile
import unittest

from build_test import build, run


class BuildTest(unittest.TestCase):
    def test_real_encode_preserves_master_and_measures_variants(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'source'
            source.mkdir()
            master = source / 'test.wav'
            run('ffmpeg', '-v', 'error', '-f', 'lavfi', '-i',
                'sine=frequency=440:duration=2', str(master))
            before = hashlib.sha256(master.read_bytes()).hexdigest()
            data = build(source, root)
            track = data['tracks'][0]
            self.assertEqual(before, hashlib.sha256(master.read_bytes()).hexdigest())
            self.assertEqual([32, 24, 20, 16], [v['bitrateKbps'] for v in track['variants']])
            for variant in track['variants']:
                path = root / variant['path']
                probe = run('ffprobe', '-v', 'error', '-show_entries',
                            'stream=codec_name,channels', '-of', 'json', str(path)).stdout
                self.assertIn('"opus"', probe)
                self.assertIn('"channels": 1', probe)
                self.assertAlmostEqual(2, variant['durationSeconds'], delta=.05)
                self.assertEqual(path.stat().st_size, variant['sizeBytes'])
                self.assertAlmostEqual(variant['sizeBytes'] / variant['durationSeconds'] * .06,
                                       variant['kbPerMinute'])

    def test_empty_sources_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, 'No supported masters'):
                build(Path(directory), Path(directory))
