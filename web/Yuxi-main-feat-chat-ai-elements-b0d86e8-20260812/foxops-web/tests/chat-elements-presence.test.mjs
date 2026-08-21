import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const aiDir = path.join(root, 'src/components/ai-elements')

test('ai-elements 组件目录已安装', () => {
  assert.ok(fs.existsSync(aiDir), '缺少 src/components/ai-elements')
  for (const name of ['conversation', 'message', 'reasoning', 'tool', 'prompt-input']) {
    const hits = fs
      .readdirSync(aiDir)
      .some((f) => f.includes(name) || fs.existsSync(path.join(aiDir, name)))
    assert.ok(hits, `缺少 ai-elements: ${name}`)
  }
})

test('chat agent 不再依赖 vue-element-plus-x BubbleList', () => {
  const src = fs.readFileSync(path.join(root, 'src/views/chat/agent/index.vue'), 'utf8')
  assert.doesNotMatch(src, /BubbleList/)
  assert.match(src, /ElementsConversation|ai-elements/)
})
