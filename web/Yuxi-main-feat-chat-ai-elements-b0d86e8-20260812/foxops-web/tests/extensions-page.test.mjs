import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'

test('extensions list pages use article-list style page-content shell', async () => {
  for (const file of [
    '../src/views/extensions/knowledge/index.vue',
    '../src/views/extensions/tools/index.vue',
    '../src/views/extensions/mcp/index.vue',
    '../src/views/extensions/skills/index.vue'
  ]) {
    const source = await readFile(new URL(file, import.meta.url), 'utf8')
    assert.match(source, /page-content/, `missing page-content in ${file}`)
    assert.match(source, /!mb-5/, `missing article-list spacing in ${file}`)
  }
})

test('extension toolbar uses article-list top row layout', async () => {
  const source = await readFile(
    new URL('../src/components/extensions/common/ExtensionToolbar.vue', import.meta.url),
    'utf8'
  )
  assert.match(source, /ElRow/)
  assert.match(source, /ElCol/)
  assert.match(source, /justify="space-between"/)
})

test('auth maps admin roles and user store exposes isAdmin', async () => {
  const auth = await readFile(new URL('../src/api/auth.ts', import.meta.url), 'utf8')
  const user = await readFile(new URL('../src/store/modules/user.ts', import.meta.url), 'utf8')
  assert.match(auth, /R_ADMIN/)
  assert.match(auth, /R_SUPER/)
  assert.match(auth, /role:\s*yuxi\.role/)
  assert.match(user, /isAdmin/)
  assert.match(user, /role === 'admin' \|\| role === 'superadmin'/)
})

test('extension common and feature components exist', async () => {
  const files = [
    '../src/components/extensions/common/ExtensionToolbar.vue',
    '../src/components/extensions/common/ExtensionCardGrid.vue',
    '../src/components/extensions/common/ExtensionInfoCard.vue',
    '../src/components/extensions/common/ExtensionEmptyState.vue',
    '../src/components/extensions/common/ExtensionDetailHeader.vue',
    '../src/components/extensions/common/ShareScopeForm.vue',
    '../src/components/extensions/tools/ToolsCardList.vue',
    '../src/components/extensions/tools/ToolDetailDialog.vue',
    '../src/components/extensions/mcp/McpCardList.vue',
    '../src/components/extensions/mcp/McpFormDialog.vue',
    '../src/components/extensions/mcp/McpToolList.vue',
    '../src/components/extensions/skills/SkillCardList.vue',
    '../src/components/extensions/skills/SkillFileManager.vue',
    '../src/components/extensions/knowledge/KnowledgeCardList.vue',
    '../src/components/extensions/knowledge/KnowledgeCreateDialog.vue',
    '../src/components/extensions/knowledge/KnowledgeDocumentTable.vue',
    '../src/components/extensions/knowledge/KnowledgeQueryPanel.vue',
    '../src/components/extensions/knowledge/KnowledgeGraphPanel.vue',
    '../src/components/extensions/knowledge/KnowledgeMindMapPanel.vue',
    '../src/components/extensions/knowledge/KnowledgeEvaluationPanel.vue',
    '../src/views/extensions/knowledge/detail.vue',
    '../src/views/extensions/mcp/detail.vue',
    '../src/views/extensions/skill/detail.vue'
  ]
  for (const file of files) {
    const source = await readFile(new URL(file, import.meta.url), 'utf8')
    assert.ok(source.includes('<template>'), `invalid component ${file}`)
  }
})
