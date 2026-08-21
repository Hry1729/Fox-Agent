import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { test } from 'node:test'

test('knowledge graph panel matches Yuxi empty-state chrome', async () => {
  const src = await readFile(
    new URL('../src/components/extensions/knowledge/KnowledgeGraphPanel.vue', import.meta.url),
    'utf8'
  )

  assert.match(src, /搜索实体/)
  assert.match(src, /待索引/)
  assert.match(src, /索引管理/)
  assert.match(src, /图谱设置/)
  assert.match(src, /最大节点数/)
  assert.match(src, /搜索深度/)
  assert.match(src, /排除 Chunk 节点/)
  assert.match(src, /暂无知识图谱/)
  assert.match(src, /配置抽取器/)
  assert.match(src, /总 Chunk/)
  assert.match(src, /待构建/)
  assert.match(src, /floating-panel/)
  assert.match(src, /configureGraphBuild/)
  assert.match(src, /getV2Models\('chat'\)/)
  assert.match(src, /ElSelect/)
  assert.match(src, /form-grid-params/)
  assert.match(src, /GraphCanvas/)
  assert.match(src, /viewMode/)
  assert.match(src, /图谱/)
  assert.match(src, /列表/)
})

test('graph canvas uses antv g6', async () => {
  const src = await readFile(
    new URL(
      '../src/components/extensions/knowledge/graph/GraphCanvas.vue',
      import.meta.url
    ),
    'utf8'
  )
  assert.match(src, /from '@antv\/g6'/)
  assert.match(src, /type: 'd3-force'/)
  assert.match(src, /source_id/)
  assert.match(src, /useSettingStore/)
})
