import test from 'node:test'
import assert from 'node:assert/strict'
import { normalizeFoxToolResultForPi } from '../src/tool-adapter.mjs'

const MARKER = 'FOX-F1-MIDDLE-MARKER-7c1f9a2d-NEVER-DELIVERED'
const body = () => 'H'.repeat(15_000) + MARKER + 'T'.repeat(15_000)

test('F1 Legacy: actual Pi adapter preserves a 30 KB Host read result whole', () => {
  // The Host read seam returns a Pi-shaped envelope with content and details.
  // The adapter takes its isPiToolResult branch for this shape.
  const hostResult = {
    content: [
      { type: 'text', text: body() },
      { type: 'text', text: 'readVersion: sha256:example. Use this as expectedVersion for write_file/edit_file.' },
    ],
    details: { readVersion: 'sha256:example', truncated: false },
  }
  const delivered = normalizeFoxToolResultForPi('read', hostResult, {
    runId: 'legacy-run', toolCallId: 'read-big',
    storage: { stored: true, storedBytes: body().length, retrievableBytes: body().length },
  })
  assert.equal(delivered.content[0].text, hostResult.content[0].text)
  assert.ok(delivered.content[0].text.includes(MARKER))
  assert.equal(delivered.details.readVersion, hostResult.details.readVersion)
})

test('F1 Legacy: Pi-shaped read remains whole even beyond the generic result gate', () => {
  const source = 'A'.repeat(140_000) + MARKER
  const hostResult = { content: [{ type: 'text', text: source }], details: { truncated: false } }
  const delivered = normalizeFoxToolResultForPi('read', hostResult, {
    runId: 'legacy-run', toolCallId: 'read-larger',
  })
  assert.equal(delivered.content[0].text, source)
  assert.equal(delivered.content[0].text.includes(MARKER), true)
})
