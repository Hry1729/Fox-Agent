import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'

test('workspace page orchestrates personal and knowledge file workflows', async () => {
  const source = await readFile(
    new URL('../src/views/workspace/index.vue', import.meta.url),
    'utf8'
  )
  for (const token of [
    'workspaceApi.getTree',
    'workspaceApi.getFile',
    'workspaceApi.saveFile',
    'workspaceApi.deletePath',
    'workspaceApi.createDirectory',
    'workspaceApi.uploadFiles',
    'workspaceApi.downloadFile',
    'workspaceApi.getKnowledgeTree',
    'workspaceApi.getKnowledgeFile',
    'workspaceApi.downloadKnowledgeFile',
    'knowledgeApi.getAccessibleDatabases'
  ])
    assert.ok(source.includes(token), `missing ${token}`)
})

test('workspace page opens file preview in a large modal dialog', async () => {
  const source = await readFile(
    new URL('../src/views/workspace/index.vue', import.meta.url),
    'utf8'
  )
  assert.match(source, /MAX_UPLOAD_FILES\s*=\s*50/)
  assert.match(source, /previewRequestId/)
  assert.match(source, /revokeObjectURL/)
  assert.match(source, /previewModalVisible\.value\s*=\s*true/)
  assert.match(source, /workspace-file-preview-modal/)
  assert.match(source, /width="92vw"/)
  assert.match(source, /:readonly="isKnowledgeSource"/)
  assert.doesNotMatch(source, /showInlinePreview/)
  assert.doesNotMatch(source, /WorkspacePreviewPane/)
  assert.match(source, /requiresOriginal/)
  assert.match(source, /\['image', 'pdf', 'spreadsheet', 'office'\]/)
  assert.match(source, /databaseLoadError/)
})
