import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { test } from 'node:test'

test('knowledge create dialog drops AI description and matches MCP form density', async () => {
  const create = await readFile(
    new URL('../src/components/extensions/knowledge/KnowledgeCreateDialog.vue', import.meta.url),
    'utf8'
  )
  assert.doesNotMatch(create, /AI 生成描述/)
  assert.doesNotMatch(create, /generateDescription/)
  assert.match(create, /kb-form-grid/)
  assert.match(create, /top="5vh"/)
  assert.match(create, /ExtensionDialogHeader/)
  assert.match(create, /extension-dialog-header/)

  const mcp = await readFile(
    new URL('../src/components/extensions/mcp/McpFormDialog.vue', import.meta.url),
    'utf8'
  )
  assert.match(mcp, /top="5vh"/)
  assert.match(mcp, /ExtensionDialogHeader/)
  assert.match(mcp, /extension-dialog-header/)
  assert.match(mcp, /:show-close="false"/)
  assert.doesNotMatch(mcp, /extension-dialog-compact/)
})

test('skill cards open preview on click and manage via footer action', async () => {
  const skills = await readFile(
    new URL('../src/components/extensions/skills/SkillCardList.vue', import.meta.url),
    'utf8'
  )
  assert.match(skills, /handleCardClick/)
  assert.match(skills, /openPreview/)
  assert.match(skills, /skill-manage-btn/)
  assert.match(skills, /\r?\n\s*管理\r?\n/)
  assert.doesNotMatch(skills, /去管理/)
  assert.doesNotMatch(skills, /预览</)
  assert.doesNotMatch(skills, /ElSwitch/)

  const preview = await readFile(
    new URL('../src/components/extensions/skills/SkillPreviewDialog.vue', import.meta.url),
    'utf8'
  )
  assert.match(preview, /skill-preview-panel/)
  assert.match(preview, /skill-preview-header/)
  assert.match(preview, /skill-preview-body/)
  assert.match(preview, /skill-preview-footer/)
  assert.match(preview, /去管理/)
  assert.match(preview, /list-style-type: decimal/)
  assert.match(preview, /renderedHtml/)
})

test('skill detail page uses two-card layout like knowledge detail', async () => {
  const detail = await readFile(
    new URL('../src/views/extensions/skill/detail.vue', import.meta.url),
    'utf8'
  )
  assert.match(detail, /detail-title-card/)
  assert.match(detail, /detail-content-card/)
  assert.match(detail, /detail-content-plain/)
  assert.match(detail, /detail-nav/)
  assert.match(detail, /代码管理/)
  assert.match(detail, /范围和依赖/)
  assert.match(detail, /section-card/)

  const dependencyPanel = await readFile(
    new URL('../src/components/extensions/skills/SkillDependencyPanel.vue', import.meta.url),
    'utf8'
  )
  assert.match(dependencyPanel, /dep-grid/)
  assert.match(dependencyPanel, /dep-card/)
  assert.match(dependencyPanel, /grid-template-columns: repeat\(3/)
  assert.doesNotMatch(detail, /label: '生效范围'/)
  assert.doesNotMatch(detail, /label: '依赖管理'/)
  assert.doesNotMatch(detail, /ExtensionDetailHeader/)

  const fileManager = await readFile(
    new URL('../src/components/extensions/skills/SkillFileManager.vue', import.meta.url),
    'utf8'
  )
  assert.match(fileManager, /项目结构/)
  assert.match(fileManager, /FileTypeIcon/)
  assert.match(fileManager, /renderMarkdown/)
  assert.match(fileManager, /canPreviewMarkdown/)
})
