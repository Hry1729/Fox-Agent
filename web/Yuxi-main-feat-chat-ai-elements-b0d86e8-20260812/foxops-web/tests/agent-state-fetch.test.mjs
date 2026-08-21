import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const runStreamPath = path.join(root, 'src/hooks/agent/useAgentRunStream.js')

test('useAgentRunStream 拉取 agent_state 时只传 threadId，不传 agent slug', () => {
  const source = fs.readFileSync(runStreamPath, 'utf8')

  assert.doesNotMatch(
    source,
    /fetchAgentState\(\s*unref\(currentAgentId\)\s*,/,
    '不应把 currentAgentId 当成 threadId 传给 getAgentState'
  )
  assert.match(source, /fetchAgentState\(\s*threadId\s*\)/)
})
