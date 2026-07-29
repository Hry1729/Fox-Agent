import assert from 'node:assert/strict'
import test from 'node:test'
import { parseArguments, validateBaseUrl, validateContentRange, validateSourceHeaders } from './verify-knowledge-preview-contract.mjs'

test('parses required verifier arguments without accepting secrets', () => {
  assert.deepEqual(parseArguments(['--', '--base-url', 'http://127.0.0.1:5010/', '--kb-id', 'kb-1', '--file-id', 'file-1']), {
    help: false,
    baseUrl: 'http://127.0.0.1:5010',
    kbId: 'kb-1',
    fileId: 'file-1',
  })
  assert.throws(() => parseArguments(['--base-url', 'http://127.0.0.1:5010', '--kb-id', 'kb-1', '--file-id', 'file-1', '--token', 'secret']), /未知参数：--token/)
  assert.throws(() => validateBaseUrl('http://user:secret@example.com'), /不能包含凭证/)
})

test('validates original source and range headers', () => {
  const headers = new Headers({
    'content-length': '4096',
    'accept-ranges': 'bytes',
    'x-source-revision': 'sha256:abc',
    'x-available-variants': 'original',
  })
  assert.deepEqual(validateSourceHeaders(headers), { size: 4096, sourceRevision: 'sha256:abc' })
  headers.delete('x-source-revision')
  assert.throws(() => validateSourceHeaders(headers), /x-source-revision/)
})

test('rejects non-exact content ranges', () => {
  assert.doesNotThrow(() => validateContentRange('bytes 0-31/4096', 0, 31, 4096))
  assert.throws(() => validateContentRange('bytes 0-63/4096', 0, 31, 4096), /不匹配/)
  assert.throws(() => validateContentRange(null, 0, 31, 4096), /格式无效/)
})
