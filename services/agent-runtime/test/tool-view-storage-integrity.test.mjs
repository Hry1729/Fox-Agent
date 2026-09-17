import test from 'node:test'
import assert from 'node:assert/strict'
import { boundToolText, resultIsRetrievable } from '../src/tool-view.mjs'

test('every structured projection preserves bytes without verified complete retrieval', () => {
  const text = JSON.stringify({ text: 'A'.repeat(5000) + 'MUST_SURVIVE' + 'B'.repeat(5000) })
  const properties = JSON.stringify(Object.fromEntries(Array.from({ length: 2000 }, (_, i) => [`column${i}`, i])))
  for (const storage of [null, { stored: false }, { stored: true, storedBytes: 100, retrievableBytes: 100 }]) {
    for (const input of [text, properties]) {
      assert.equal(boundToolText('read', { text: input, resultRef: 'fox-result://run/call', storage }), input)
    }
  }
  assert.equal(resultIsRetrievable({ stored: true, storedBytes: 200000 }, 200000), false)
})

test('complete storage and a scoped reference permit a bounded structured view', () => {
  const text = JSON.stringify({ text: 'A'.repeat(200000) })
  const size = Buffer.byteLength(text)
  const storage = { stored: true, storedBytes: size, retrievableBytes: size }
  assert.equal(boundToolText('read', { text, storage }), text)
  const view = boundToolText('read', { text, resultRef: 'fox-result://run/call', storage })
  assert.ok(Buffer.byteLength(view) <= 9000)
  assert.equal(JSON.parse(view).foxModelView.retrievable, true)
})
