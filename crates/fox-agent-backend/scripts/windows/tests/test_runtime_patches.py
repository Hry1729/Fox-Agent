import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import runtime_patches as patches


class RuntimePatchSafetyTest(unittest.TestCase):
    def test_unknown_version_does_not_modify_the_staged_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            package = Path(tmp) / patches.PATCHES[0]['package']
            (package / 'dist').mkdir(parents=True)
            (package / 'package.json').write_text('{"version":"0.0.35"}', encoding='utf-8')
            source = package / 'dist/index.js'
            source.write_bytes(b'original')
            with self.assertRaisesRegex(ValueError, 'unvalidated version'):
                patches.apply(tmp)
            self.assertEqual(source.read_bytes(), b'original')

    def test_unknown_hash_does_not_install_helpers_or_patch_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            package = Path(tmp) / patches.PATCHES[0]['package']
            (package / 'dist').mkdir(parents=True)
            (package / 'package.json').write_text('{"version":"0.0.34"}', encoding='utf-8')
            source = package / 'dist/index.js'
            source.write_bytes(b'locally changed source')
            with self.assertRaisesRegex(ValueError, 'source hash differs'):
                patches.apply(tmp)
            self.assertEqual(source.read_bytes(), b'locally changed source')
            self.assertFalse((package / 'dist/fox-turn-outcome.mjs').exists())
            self.assertFalse((Path(tmp) / 'model-adapters.json').exists())

    def test_helper_sources_match_host_build_time_pins(self):
        pins = json.loads((patches.ASSETS / 'pins.json').read_text(encoding='utf-8'))['files']
        for item in patches.PATCHES:
            if 'helper' not in item:
                continue
            source, dest = item['helper']
            data = (patches.ASSETS / source).read_bytes().replace(b'\r\n', b'\n')
            self.assertEqual(hashlib.sha256(data).hexdigest(), pins[item['package'] + '/' + dest])


if __name__ == '__main__':
    unittest.main()
