import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { test } from 'node:test'

test('hidden detail routes update parent worktab instead of skipping', async () => {
  const source = await readFile(
    new URL('../src/utils/navigation/worktab.ts', import.meta.url),
    'utf8'
  )
  assert.match(source, /meta\.isHideTab && meta\.activePath/)
  assert.match(source, /setHiddenDetailWorktab/)
  assert.match(source, /parentPath:\s*activePath/)
  assert.match(source, /findRelatedTabIndex/)
})

test('worktab store can match tabs by parentPath', async () => {
  const source = await readFile(
    new URL('../src/store/modules/worktab.ts', import.meta.url),
    'utf8'
  )
  assert.match(source, /tab\.parentPath/)
  assert.match(source, /t\.parentPath === tab\.parentPath/)
})

test('WorkTab type includes parentPath for detail tab reuse', async () => {
  const source = await readFile(new URL('../src/types/store/index.ts', import.meta.url), 'utf8')
  assert.match(source, /parentPath\?: string/)
})
