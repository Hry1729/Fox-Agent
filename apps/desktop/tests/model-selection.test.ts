import { expect, test } from 'bun:test'
import { conversationModelOverride } from '../src/features/chat/model-selection'
import type { AgentRecord } from '../src/features/conversations/model/types'

const remote = { id: 'yuxi:default-chatbot', runtimeType: 'yuxi', configurableItems: { model: { kind: 'llm' } } } as AgentRecord
const catalog = [{ spec: 'siliconflow-cn:deepseek-ai/DeepSeek-V4-Flash' }]

test('new remote messages and retries never inherit local models or default labels', () => {
  for (const stale of ['sensenova-6.8-flash-lite', '知识库默认模型', '', undefined]) {
    expect(conversationModelOverride(remote, stale, catalog)).toBeUndefined()
  }
  expect(conversationModelOverride(remote, catalog[0].spec, catalog)).toBe(catalog[0].spec)
  expect(conversationModelOverride({ ...remote, configurableItems: {} }, catalog[0].spec, catalog)).toBeUndefined()
})

test('missing remote metadata or catalog cannot turn a local ID into a remote override', () => {
  expect(conversationModelOverride(null, 'sensenova-6.8-flash-lite', catalog)).toBeUndefined()
  expect(conversationModelOverride(remote, 'sensenova-6.8-flash-lite', [])).toBeUndefined()
  expect(conversationModelOverride({ ...remote, runtimeType: 'pi' }, 'sensenova-6.8-flash-lite', catalog)).toBe('sensenova-6.8-flash-lite')
})
