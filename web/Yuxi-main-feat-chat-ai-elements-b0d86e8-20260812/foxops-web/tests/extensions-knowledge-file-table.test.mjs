import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { test } from 'node:test'

test('knowledge document table uses ArtTable like examples/tables', async () => {
  const src = await readFile(
    new URL('../src/components/extensions/knowledge/KnowledgeDocumentTable.vue', import.meta.url),
    'utf8'
  )
  assert.match(src, /<ArtTable/)
  assert.match(src, /art-table-card/)
  assert.match(src, /ArtButtonTable/)
  assert.match(src, /useSlot:\s*true/)
  assert.match(src, /unwrapApiData/)
  assert.match(src, /data\?\.entries/)
  assert.match(src, /row\?\.is_dir/)
  assert.doesNotMatch(src, /<ElTable[\s>]/)
})

test('knowledge file detail restores original Yuxi source/chunk views and spreadsheet preview', async () => {
  const src = await readFile(
    new URL(
      '../src/components/extensions/knowledge/KnowledgeFileDetailDialog.vue',
      import.meta.url
    ),
    'utf8'
  )
  for (const label of ['原文件', '解析内容', 'Chunks']) assert.match(src, new RegExp(label))
  assert.match(src, /knowledgeApi\.downloadDocument/)
  assert.match(src, /normalizePreviewResponse/)
  assert.match(src, /SpreadsheetPreview/)
})

test('default extension card arrow navigates with the card instead of swallowing clicks', async () => {
  const src = await readFile(
    new URL('../src/components/extensions/common/ExtensionInfoCard.vue', import.meta.url),
    'utf8'
  )
  assert.match(src, /onActionClick/)
  assert.match(src, /if \(!slots\.action\) onClick\(\)/)
})
