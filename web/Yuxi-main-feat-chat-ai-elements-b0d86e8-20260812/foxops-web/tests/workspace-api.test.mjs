import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'

test('workspace API declares every Yuxi workspace endpoint and blob responses', async () => {
  const source = await readFile(new URL('../src/api/workspace.ts', import.meta.url), 'utf8')
  for (const endpoint of [
    '/api/workspace/tree',
    '/api/workspace/file',
    '/api/workspace/directory',
    '/api/workspace/upload',
    '/api/workspace/download',
    '/api/workspace/knowledge/tree',
    '/api/workspace/knowledge/file',
    '/api/workspace/knowledge/download'
  ]) {
    assert.ok(source.includes(endpoint), `missing ${endpoint}`)
  }
  assert.match(source, /authenticatedBlobRequest/)
})

test('knowledge API exposes the accessible knowledge base endpoint', async () => {
  const source = await readFile(new URL('../src/api/knowledge.ts', import.meta.url), 'utf8')
  assert.ok(source.includes('/api/knowledge/databases/accessible'))
})

