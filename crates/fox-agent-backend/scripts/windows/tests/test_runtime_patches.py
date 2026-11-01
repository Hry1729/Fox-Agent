import hashlib
import json
from pathlib import Path
import shutil
import subprocess
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


FIXTURES = Path(__file__).resolve().parent / 'fixtures'

# Minimal stand-ins for the excerpt's imports (@deepseek-ai/dsh-llm, eventsource-parser).
DSH_STREAM_HARNESS = r"""
class LlmError extends Error { constructor(message, code, extra) { super(message); this.code = code; Object.assign(this, extra ?? {}); } }
const ToolCallId = (value) => value;
const ProviderRequestId = (value) => value;
const isQuotaExceededError = () => false;
const isContextWindowExceededError = () => false;
class EventSourceParserStream extends TransformStream {
  constructor({ onComment } = {}) {
    let buffer = '';
    const frame = (text, controller) => {
      let event, data = [];
      for (const line of text.split(/\r?\n/)) {
        if (line.startsWith(':')) { onComment?.(line.slice(1)); continue; }
        const at = line.indexOf(':');
        const field = at < 0 ? line : line.slice(0, at);
        const value = at < 0 ? '' : line.slice(at + 1).replace(/^ /, '');
        if (field === 'event') event = value;
        else if (field === 'data') data.push(value);
      }
      if (data.length) controller.enqueue({ event, data: data.join('\n') });
    };
    super({
      transform(chunk, controller) {
        buffer += chunk;
        let match;
        while ((match = /\r?\n\r?\n/.exec(buffer))) { frame(buffer.slice(0, match.index), controller); buffer = buffer.slice(match.index + match[0].length); }
      },
    });
  }
}
"""

# A SenseNova-style stream: content_block_start without "thinking" / "text",
# "signature": null, the content arriving in deltas.
DSH_STREAM_DRIVER = r"""
const sse = [
  ['message_start', { type: 'message_start', message: { usage: { input_tokens: 3, output_tokens: 0 } } }],
  ['content_block_start', { type: 'content_block_start', index: 0, content_block: { type: 'thinking', signature: null } }],
  ['content_block_delta', { type: 'content_block_delta', index: 0, delta: { type: 'thinking_delta', thinking: '先想' } }],
  ['content_block_stop', { type: 'content_block_stop', index: 0 }],
  ['content_block_start', { type: 'content_block_start', index: 1, content_block: { type: 'text' } }],
  ['content_block_delta', { type: 'content_block_delta', index: 1, delta: { type: 'text_delta', text: '你好' } }],
  ['content_block_stop', { type: 'content_block_stop', index: 1 }],
  ['message_delta', { type: 'message_delta', delta: { stop_reason: 'end_turn' }, usage: { output_tokens: 5 } }],
  ['message_stop', { type: 'message_stop' }],
].map(([event, data]) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`).join('');
const body = new ReadableStream({ start(controller) { controller.enqueue(new TextEncoder().encode(sse)); controller.close(); } });
const out = [];
try {
  for await (const chunk of translate(parseSse(body, () => {}), 'model-x')) out.push(chunk);
  console.log(JSON.stringify({ ok: true, out }));
} catch (error) {
  console.log(JSON.stringify({ ok: false, message: error.message, code: error.code }));
}
"""


@unittest.skipUnless(shutil.which('node'), 'node is required to run the dsh stream fixture')
class DshMessagesStreamPatchTest(unittest.TestCase):
    """fox-runtime-dsh-messages: missing text/thinking or null signature in content_block_start."""

    def run_stream(self, source):
        with tempfile.TemporaryDirectory() as tmp:
            module = Path(tmp) / 'stream.mjs'
            module.write_text(DSH_STREAM_HARNESS + source + '\nexport { parseSse, translate };\n', encoding='utf-8')
            driver = Path(tmp) / 'driver.mjs'
            driver.write_text("import { parseSse, translate } from './stream.mjs';\n" + DSH_STREAM_DRIVER, encoding='utf-8')
            done = subprocess.run(['node', str(driver)], capture_output=True, text=True, encoding='utf-8', timeout=60)
            self.assertEqual(done.returncode, 0, done.stderr)
            return json.loads(done.stdout.strip().splitlines()[-1])

    def fixture(self):
        return (FIXTURES / 'dsh-llm-deepseek-0.2.1-rc.2-stream.js').read_text(encoding='utf-8').replace('\r\n', '\n')

    def test_upstream_rejects_the_stream(self):
        result = self.run_stream(self.fixture())
        self.assertFalse(result['ok'])
        self.assertEqual(result['code'], 'MALFORMED_RESPONSE')
        self.assertIn('expected a string field', result['message'])

    def test_patched_stream_reads_missing_fields_and_null_signature_as_empty(self):
        result = self.run_stream(patches.patch_dsh_messages(self.fixture()))
        self.assertTrue(result['ok'], result)
        out = result['out']
        deltas = [(c['type'], c.get('text')) for c in out if c['type'] in ('reasoning-delta', 'text-delta')]
        self.assertEqual(deltas, [('reasoning-delta', '先想'), ('text-delta', '你好')])
        ends = [c['block'] for c in out if c['type'] == 'block-end']
        self.assertEqual(ends, [{'type': 'reasoning', 'text': '先想'}, {'type': 'text', 'text': '你好'}])
        finish = out[-1]
        self.assertEqual(finish['type'], 'finish')
        self.assertEqual(finish['reason'], {'kind': 'stop'})
        self.assertEqual(finish['replayState']['blocks'], [{'type': 'reasoning', 'signature': ''}, {'type': 'text'}])

    def test_patch_anchors_match_once_and_the_entry_is_pinned(self):
        patched = patches.patch_dsh_messages(self.fixture())
        with self.assertRaisesRegex(ValueError, 'anchor differs'):
            patches.patch_dsh_messages(patched)
        item = next(i for i in patches.PATCHES if i['id'] == 'dsh-messages-stream')
        self.assertEqual(item['version'], '0.2.1-rc.2')
        pins = json.loads((patches.ASSETS / 'pins.json').read_text(encoding='utf-8'))['files']
        self.assertIn(item['package'] + '/' + item['file'], pins)


if __name__ == '__main__':
    unittest.main()
