import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { test } from 'node:test'

test('knowledge query panel uses left/right three-card layout', async () => {
  const panel = await readFile(
    new URL('../src/components/extensions/knowledge/KnowledgeQueryPanel.vue', import.meta.url),
    'utf8'
  )
  const config = await readFile(
    new URL('../src/components/extensions/knowledge/KnowledgeSearchConfig.vue', import.meta.url),
    'utf8'
  )

  assert.match(panel, /query-layout/)
  assert.match(panel, /query-input-card/)
  assert.match(panel, /query-result-card/)
  assert.match(panel, /query-config-pane/)
  assert.match(panel, /示例问题/)
  assert.match(panel, /检索配置/)
  assert.match(panel, /KnowledgeSearchConfig/)

  assert.match(config, /params\?\.options/)
  assert.match(config, /depend_on/)
  assert.match(config, /include_distances/)
  assert.match(config, /启用/)
  assert.match(config, /关闭/)
})
