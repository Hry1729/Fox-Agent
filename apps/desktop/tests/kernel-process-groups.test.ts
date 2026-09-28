import { expect, test } from 'bun:test'
import { projectKernelGroups } from '../src/features/chat/runtime-process-groups'
import { groupAssistantContinuations } from '../src/features/chat/runtime-timeline-performance'
import type { ConversationMessage, RunEventRecord } from '../src/features/conversations/model/types'

const message = (checkpoint: number, content: string): ConversationMessage => ({
  id: `kernel-message:run:${checkpoint}`, runId: 'run', conversationId: 'c', role: 'assistant', kind: 'text',
  content, status: 'completed', ordinal: checkpoint, createdAt: checkpoint, updatedAt: checkpoint,
})
const event = (seq: number, type: string, data: Record<string, unknown>): RunEventRecord => ({
  runId: 'run', seq, eventType: type, event: { type, ...data }, createdAt: 1,
})
const events = [
  // Host compatibility insertion order is deliberately different from round order.
  event(1, 'tool.started', { toolCallId: 'search', tool: 'grep', kernelCheckpointSeq: '2' }),
  event(2, 'reasoning.delta', { source: 'kernel-model:2', delta: '先分析' }),
  event(3, 'tool.completed', { toolCallId: 'search', tool: 'grep', kernelCheckpointSeq: '2', result: 'found' }),
  event(4, 'reasoning.delta', { source: 'kernel-model:12', delta: '再分析' }),
]

test('Kernel round ownership orders reasoning, response, tools and the next response once', () => {
  const messages = [message(2, '先搜索'), message(12, '结论')]
  const grouped = groupAssistantContinuations(messages)[0]
  expect(grouped.kernelMessages).toHaveLength(2)
  const groups = projectKernelGroups('run', grouped.kernelMessages!, [...events, events[0]])!
  expect(groups.map(group => group.kind)).toEqual(['process', 'response', 'process', 'response'])
  expect(groups.filter(group => group.kind === 'response').map(group => group.text)).toEqual(['先搜索', '结论'])
  expect(groups[0].kind === 'process' && groups[0].events[0].event.delta).toBe('先分析')
  expect(groups[2].kind === 'process' && groups[2].events.map(item => item.eventType)).toEqual(['tool.started', 'tool.completed', 'reasoning.delta'])
})

test('same-length and shorter Kernel previews replace text and reasoning with stable keys', () => {
  const original = message(12, '旧答案')
  original.kernelPreview = { checkpointSeq: 12, revision: 1, reasoning: '旧推理' }
  const first = projectKernelGroups('run', [message(2, '先搜索'), original], events)!
  for (const text of ['新答案', '短']) {
    const next = projectKernelGroups('run', [message(2, '先搜索'), { ...original, content: text,
      kernelPreview: { checkpointSeq: 12, revision: 2, reasoning: '新推理' } }], events)!
    expect(next.map(group => group.key)).toEqual(first.map(group => group.key))
    expect(JSON.stringify(next)).not.toContain('旧推理')
    expect(JSON.stringify(next)).not.toContain('再分析')
    expect(next.at(-1)).toMatchObject({ kind: 'response', text })
  }
})

test('tool-only rounds, empty replacements, failure notices and foreign events stay bounded', () => {
  const preview = { ...message(12, ''), kernelPreview: { checkpointSeq: 12, revision: 2, reasoning: '' } }
  const groups = projectKernelGroups('run', [preview], [...events, { ...events[0], runId: 'foreign' }, event(9, 'run.failed', {})])!
  expect(groups.filter(group => group.kind === 'response')).toHaveLength(0)
  expect(JSON.stringify(groups)).not.toContain('再分析')
  expect(groups.at(-1)).toMatchObject({ kind: 'notice', label: '运行失败' })
  expect(groups.filter(group => group.kind === 'process')).toHaveLength(1)
  expect(projectKernelGroups('run', [preview], [event(4, 'reasoning.delta', { source: 'kernel-model:12', delta: '过期推理' })])).toEqual([])
})

test('missing tool ownership or foreign/mixed messages fall back instead of guessing', () => {
  expect(projectKernelGroups('run', [message(2, '答')], [event(1, 'tool.started', { toolCallId: 'unknown' })])).toBeNull()
  expect(projectKernelGroups('run', [{ ...message(2, '答'), runId: 'other' }], [])).toBeNull()
})

test('Mermaid fences remain intact per Kernel response including streaming prefixes', () => {
  const source = '说明\n```mermaid\nflowchart TD\nA[开始] --> B[结束]\n```\n结论'
  for (const content of [source.slice(0, 32), source]) {
    const groups = projectKernelGroups('run', [message(2, content)], [])!
    expect(groups).toEqual([{ kind: 'response', key: 'run:kernel:2:response', text: content }])
  }
})
