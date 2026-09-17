// Size-limit layering at the REAL legacy Pi consumer (tool-adapter).
//
// The 64 KiB `MAX_RESULT_BYTES` bounds only attachment_compute's inline
// `result` field (a large result is spilled to a `files[]` compute artifact and
// replaced by a bounded summary). The OUTER response carries `files[]` with the
// stable `compute-artifact:` id the model passes to office_import_data; it is
// not subject to the 64 KiB result cap. The Pi adapter publishes every result
// up to 120_000 chars intact, and beyond that either bounds it honestly (with a
// Host-verified retrieval fact) or keeps it whole — never a silent character
// cut that could drop the files reference.
import test from 'node:test'
import assert from 'node:assert/strict'
import { normalizeFoxToolResultForPi } from '../src/tool-adapter.mjs'

const sha = 'a'.repeat(64)

test('explicit saveFile CSV: small inline result + large file id is published intact', () => {
  const result = {
    result: { rows: 927, columns: 12, saved: 'agv.csv' },
    files: [{
      id: `compute-artifact:${sha}`,
      displayName: 'agv.csv',
      bytes: 113_500,
      sha256: sha,
      mediaType: 'text/csv',
      path: 'C:/sessions/attachment-compute/x/outputs/agv.csv',
    }],
    attachmentCount: 1,
    computedBy: 'fox-quickjs',
  }
  const view = normalizeFoxToolResultForPi('attachment_compute', result, { runId: 'run-1', toolCallId: 'call-1' })
  assert.equal(view.details.outputTruncated, false)
  const published = JSON.parse(view.content[0].text)
  assert.equal(published.files[0].id, result.files[0].id, 'the model keeps the exact office_import_data artifactId')
  assert.equal(published.files[0].bytes, 113_500, 'file metadata is full-size even though result is small')
  assert.equal(published.result.rows, 927)
})

test('auto-spill: a near-64KiB result summary still keeps the outer files reference', () => {
  // A maximally large LEGITIMATE inline result: the Host shrinks the summary so
  // it is at/below the 64 KiB result cap, while the outer response also carries
  // the files[] reference. The adapter gate is on the outer envelope (120k
  // chars), so even a near-cap result keeps its files id intact.
  const bigSample = Array.from({ length: 12 }, (_, i) => ({
    id: i,
    tag: 'agv-task-' + String(i).padStart(5, '0'),
    blob: 'x'.repeat(5_000),
  }))
  const inner = {
    summary: {
      storedBytes: 200_000,
      rows: 6000,
      columns: 12,
      sampleRows: bigSample,
      sampleTruncated: true,
      complete: false,
      storedAs: 'compute-artifact',
      artifactId: `compute-artifact:${'b'.repeat(64)}`,
      artifactName: 'large-result.json',
    },
  }
  const result = {
    result: inner,
    files: [{
      id: `compute-artifact:${'b'.repeat(64)}`,
      displayName: 'large-result.json',
      bytes: 200_000,
      sha256: 'b'.repeat(64),
      mediaType: 'application/json',
    }],
  }
  const innerBytes = Buffer.byteLength(JSON.stringify(inner), 'utf8')
  const serialized = JSON.stringify(result)
  assert.ok(innerBytes > 56 * 1024, `inner summary is near the cap (${innerBytes} B)`)
  assert.ok(innerBytes <= 64 * 1024, 'the Host keeps the inline result at/below its 64 KiB cap')
  assert.ok(Buffer.byteLength(serialized, 'utf8') < 120_000, 'outer envelope is under the adapter pass-through gate')
  const view = normalizeFoxToolResultForPi('attachment_compute', result, { runId: 'run-1', toolCallId: 'call-2' })
  assert.equal(view.details.outputTruncated, false)
  const published = JSON.parse(view.content[0].text)
  assert.equal(published.files[0].id, result.files[0].id, 'outer files id survives a result that itself hit the 64 KiB layer')
  assert.equal(published.result.summary.complete, false)
})

test('no silent hard cap: an oversized result without a retrieval fact is kept whole', () => {
  const result = { result: 'z'.repeat(140_000), files: [{ id: `compute-artifact:${'c'.repeat(64)}`, displayName: 'k.csv' }] }
  const serialized = JSON.stringify(result)
  const view = normalizeFoxToolResultForPi('attachment_compute', result, { runId: 'run-1', toolCallId: 'call-3' })
  // Without a Host storage fact nothing is dropped or character-cut.
  assert.equal(view.details.outputTruncated, false)
  assert.equal(view.content[0].text, serialized)
  assert.ok(view.content[0].text.includes(result.files[0].id))
})
