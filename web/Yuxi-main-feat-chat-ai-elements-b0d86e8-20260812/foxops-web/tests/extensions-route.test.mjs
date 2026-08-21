import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'

test('extensions uses ArtDesignPro parent layout with sidebar children', async () => {
  const source = await readFile(
    new URL('../src/router/modules/extensions.ts', import.meta.url),
    'utf8'
  )
  assert.match(source, /name:\s*'Extensions'/)
  assert.match(source, /path:\s*'\/extensions'/)
  assert.match(source, /component:\s*'\/index\/index'/)
  assert.match(source, /title:\s*'智能体扩展'/)
  assert.match(source, /path:\s*'knowledge'/)
  assert.match(source, /path:\s*'tools'/)
  assert.match(source, /path:\s*'mcp'/)
  assert.match(source, /path:\s*'skills'/)
  assert.match(source, /component:\s*'\/extensions\/knowledge\/index'/)
  assert.match(source, /component:\s*'\/extensions\/tools\/index'/)
  assert.match(source, /component:\s*'\/extensions\/mcp\/index'/)
  assert.match(source, /component:\s*'\/extensions\/skills\/index'/)
})

test('extension detail routes are hidden children with activePath', async () => {
  const source = await readFile(
    new URL('../src/router/modules/extensions.ts', import.meta.url),
    'utf8'
  )
  assert.match(source, /path:\s*'knowledgebase\/:kbId'/)
  assert.match(source, /path:\s*'mcp\/:slug'/)
  assert.match(source, /path:\s*'skill\/:slug'/)
  assert.match(source, /activePath:\s*'\/extensions\/knowledge'/)
  assert.match(source, /activePath:\s*'\/extensions\/mcp'/)
  assert.match(source, /activePath:\s*'\/extensions\/skills'/)
  assert.equal((source.match(/isHide:\s*true/g) || []).length >= 3, true)
})

test('extensions appears after agent management in sidebar modules', async () => {
  const source = await readFile(new URL('../src/router/modules/index.ts', import.meta.url), 'utf8')
  assert.match(source, /import \{ extensionsRoutes \} from '\.\/extensions'/)
  assert.match(source, /workspaceRoutes,\s*agentManageRoutes,\s*extensionsRoutes,/)
  assert.doesNotMatch(source, /extensionDetailRoutes/)
})
