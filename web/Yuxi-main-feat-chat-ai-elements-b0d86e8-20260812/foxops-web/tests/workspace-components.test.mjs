import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'

const read = (path) => readFile(new URL(path, import.meta.url), 'utf8')

test('workspace sidebar contains personal, quick access and knowledge groups', async () => {
  const source = await read('../src/components/workspace/WorkspaceSidebar.vue')
  for (const label of ['个人工作区', 'Saved Artifacts', 'Agents', '我的知识库', '共享知识库']) {
    assert.ok(source.includes(label), `missing ${label}`)
  }
  assert.match(source, /select-database/)
  assert.match(source, /created_by/)
})

test('workspace file table exposes navigation, selection and readonly actions', async () => {
  const source = await read('../src/components/workspace/WorkspaceFileTable.vue')
  for (const label of ['名称', '大小', '修改时间', '删除选中', '下载']) {
    assert.ok(source.includes(label), `missing ${label}`)
  }
  assert.match(source, /ElTable/)
  assert.match(source, /readonly/)
  assert.match(source, /page-change/)
})

test('workspace preview covers all Yuxi preview branches and editable text', async () => {
  const source = await read('../src/components/workspace/WorkspaceFilePreview.vue')
  for (const previewType of [
    'markdown',
    'text',
    'image',
    'pdf',
    'html',
    'spreadsheet',
    'office',
    'unsupported'
  ]) {
    assert.ok(source.includes(`'${previewType}'`), `missing ${previewType}`)
  }
  assert.match(source, /renderMarkdown/)
  assert.match(source, /editable/)
  assert.match(source, /save/)
  assert.match(source, /SpreadsheetPreview/)
})
