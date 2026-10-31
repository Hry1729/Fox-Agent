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


class ConsoleWindowPatchTest(unittest.TestCase):
    """fox-fix-console: windowsHide on the Node side (pi-acp, pi bundle)."""

    def test_every_pi_bundle_anchor_gets_windows_hide_once(self):
        source = ';'.join(anchor + '}' for anchor in patches.PI_BUNDLE_HIDDEN_SPAWNS)
        patched = patches.patch_pi_bundle_hidden(source)
        self.assertEqual(patched.count(',windowsHide:!0}'), len(patches.PI_BUNDLE_HIDDEN_SPAWNS))
        with self.assertRaisesRegex(ValueError, 'anchor differs'):
            patches.patch_pi_bundle_hidden(patched)  # anchors no longer match exactly

    def test_a_missing_pi_bundle_anchor_fails(self):
        source = ';'.join(anchor + '}' for anchor in patches.PI_BUNDLE_HIDDEN_SPAWNS[1:])
        with self.assertRaisesRegex(ValueError, 'anchor differs'):
            patches.patch_pi_bundle_hidden(source)

    def test_every_pi_acp_spawn_gets_windows_hide(self):
        version = 'spawnSync("pi", ["--version"], { encoding: "utf-8" })'
        source = '\n'.join([old for old, _ in patches.PI_ACP_HIDDEN_SPAWNS] + [version, version])
        patched = patches.patch_pi_acp_hidden(source)
        self.assertEqual(patched.count('windowsHide: true'), len(patches.PI_ACP_HIDDEN_SPAWNS) + 2)
        with self.assertRaisesRegex(ValueError, 'anchor differs'):
            patches.patch_pi_acp_hidden('\n'.join([old for old, _ in patches.PI_ACP_HIDDEN_SPAWNS] + [version]))

    def test_apply_is_idempotent_and_writes_nothing_on_a_bad_anchor(self):
        upstream = 'spawn(command,args,{cwd,shell:!1,stdio:["ignore","pipe","pipe"]})'
        fake_patch = lambda source: patches.replace_once(source, 'stdio:["ignore","pipe","pipe"]}', 'stdio:["ignore","pipe","pipe"],windowsHide:!0}')
        item = {"id": "fake", "package": "backends/pi/node_modules/fake", "version": "1.0.0", "file": "x.js", "sha256": patches.digest(upstream.encode()), "patch": fake_patch}
        patched = fake_patch(upstream).encode()
        original = (patches.PATCHES, patches.build_time_pins)
        patches.PATCHES = [item]
        patches.build_time_pins = lambda: {"backends/pi/node_modules/fake/x.js": patches.digest(patched)}
        try:
            with tempfile.TemporaryDirectory() as tmp:
                package = Path(tmp) / item['package']
                package.mkdir(parents=True)
                (package / 'package.json').write_text('{"version":"1.0.0"}', encoding='utf-8')
                (package / 'x.js').write_text(upstream, encoding='utf-8')
                first = patches.apply(tmp)
                self.assertEqual((package / 'x.js').read_bytes(), patched)
                second = patches.apply(tmp)  # already patched: no-op, same manifest
                self.assertEqual(first, second)
                self.assertEqual((package / 'x.js').read_bytes(), patched)
                (package / 'x.js').write_text(upstream.replace('pipe', 'PIPE'), encoding='utf-8')
                with self.assertRaisesRegex(ValueError, 'source hash differs'):
                    patches.apply(tmp)
            # A pin that the patch result does not reproduce is refused.
            patches.build_time_pins = lambda: {"backends/pi/node_modules/fake/x.js": "0" * 64}
            with tempfile.TemporaryDirectory() as tmp:
                package = Path(tmp) / item['package']
                package.mkdir(parents=True)
                (package / 'package.json').write_text('{"version":"1.0.0"}', encoding='utf-8')
                (package / 'x.js').write_text(upstream, encoding='utf-8')
                with self.assertRaisesRegex(ValueError, 'build-time pin'):
                    patches.apply(tmp)
                self.assertEqual((package / 'x.js').read_text(encoding='utf-8'), upstream)
        finally:
            patches.PATCHES, patches.build_time_pins = original


if __name__ == '__main__':
    unittest.main()
