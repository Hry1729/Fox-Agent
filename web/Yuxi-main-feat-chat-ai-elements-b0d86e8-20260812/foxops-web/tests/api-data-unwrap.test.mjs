import assert from 'node:assert/strict'
import { test } from 'node:test'
import { unwrapList } from '../src/utils/apiData.ts'

test('unwrapList handles Yuxi evaluation response envelope', () => {
  assert.deepEqual(unwrapList({ message: 'success', data: [{ id: 1 }] }), [{ id: 1 }])
  assert.deepEqual(unwrapList({ message: 'success', data: { items: [{ q: 'a' }] } }), [{ q: 'a' }])
  assert.deepEqual(unwrapList(null), [])
  assert.deepEqual(unwrapList({ foo: 'bar' }), [])
})
