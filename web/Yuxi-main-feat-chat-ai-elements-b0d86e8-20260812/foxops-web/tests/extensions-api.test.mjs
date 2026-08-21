import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'

test('tools/mcp/skills/knowledge APIs declare Yuxi endpoints', async () => {
  const tools = await readFile(new URL('../src/api/tools.ts', import.meta.url), 'utf8')
  const mcp = await readFile(new URL('../src/api/mcp.ts', import.meta.url), 'utf8')
  const skills = await readFile(new URL('../src/api/skills.ts', import.meta.url), 'utf8')
  const knowledge = await readFile(new URL('../src/api/knowledge.ts', import.meta.url), 'utf8')
  const graph = await readFile(new URL('../src/api/graph.ts', import.meta.url), 'utf8')
  const organization = await readFile(
    new URL('../src/api/organization.ts', import.meta.url),
    'utf8'
  )
  const tasker = await readFile(new URL('../src/api/tasker.ts', import.meta.url), 'utf8')

  assert.ok(tools.includes('/api/system/tools'))
  for (const endpoint of [
    '/api/system/mcp-servers',
    '/api/system/mcp-servers/${encodeURIComponent(slug)}/test',
    '/api/system/mcp-servers/${encodeURIComponent(slug)}/tools'
  ]) {
    assert.ok(mcp.includes(endpoint), `missing ${endpoint}`)
  }
  for (const endpoint of [
    '/api/skills/accessible',
    '/api/skills/import/prepare',
    '/api/skills/remote/prepare',
    '/api/skills/install-drafts/',
    '/api/system/skills/'
  ]) {
    assert.ok(skills.includes(endpoint), `missing ${endpoint}`)
  }
  for (const endpoint of [
    '/api/knowledge/databases',
    '/api/knowledge/databases/accessible',
    '/api/knowledge/types',
    '/api/knowledge/databases/${encodeURIComponent(kbId)}/documents',
    '/api/knowledge/databases/${encodeURIComponent(kbId)}/query',
    '/api/evaluation/databases/'
  ]) {
    assert.ok(knowledge.includes(endpoint), `missing ${endpoint}`)
  }
  assert.ok(graph.includes('/graph-build/status'))
  assert.ok(organization.includes('/api/departments'))
  assert.ok(organization.includes('/api/auth/users/access-options'))
  assert.ok(tasker.includes('/api/tasks'))
})
