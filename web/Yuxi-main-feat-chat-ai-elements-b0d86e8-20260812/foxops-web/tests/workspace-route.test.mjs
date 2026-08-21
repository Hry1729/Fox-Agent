import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'

test('workspace route points to the workspace page', async () => {
  const source = await readFile(new URL('../src/router/modules/workspace.ts', import.meta.url), 'utf8')
  assert.match(source, /name:\s*'Workspace'/)
  assert.match(source, /path:\s*'\/workspace'/)
  assert.match(source, /component:\s*'\/workspace\/index'/)
  assert.match(source, /title:\s*'工作区'/)
})

test('workspace appears immediately after chat in the ArtDesignPro sidebar modules', async () => {
  const source = await readFile(new URL('../src/router/modules/index.ts', import.meta.url), 'utf8')
  assert.match(source, /import \{ workspaceRoutes \} from '\.\/workspace'/)
  assert.ok(source.indexOf('chatRoutes,') < source.indexOf('workspaceRoutes,'))
  assert.equal(source.slice(source.indexOf('chatRoutes,')).match(/\w+Routes,/g)?.[1], 'workspaceRoutes,')
})

