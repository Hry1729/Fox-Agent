import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { test } from 'node:test'

test('knowledge document toolbar order matches requested left-to-right layout', async () => {
  const src = await readFile(
    new URL('../src/components/extensions/knowledge/KnowledgeDocumentTable.vue', import.meta.url),
    'utf8'
  )
  const template = src.slice(src.indexOf('<template>'), src.indexOf('</template>'))

  const upload = template.indexOf('上传文件')
  const folder = template.indexOf('新建文件夹')
  const search = template.indexOf('file-search')
  const pending = template.indexOf('待解析')
  const pendingIndex = template.indexOf('待入库')
  const filesLabel = template.indexOf('<span>文件</span>')
  const size = template.indexOf('总大小')
  const chunks = template.indexOf('Chunks')
  const tokens = template.indexOf('Tokens')

  assert.ok(upload > 0)
  assert.ok(folder > upload)
  assert.ok(search > folder)
  assert.ok(pending > search)
  assert.ok(pendingIndex > pending)
  assert.ok(filesLabel > pendingIndex)
  assert.ok(size > filesLabel)
  assert.ok(chunks > size)
  assert.ok(tokens > chunks)

  assert.doesNotMatch(template, /解析选中/)
  assert.doesNotMatch(template, /入库选中/)
  assert.match(src, /KnowledgeIndexConfigDialog/)
  assert.match(src, /row-actions/)
  assert.match(src, /\.row-actions[\s\S]*flex-wrap:\s*nowrap/)
  assert.match(src, /canShowIndexButton/)
  assert.match(src, /getIndexActionIconClass/)
})
