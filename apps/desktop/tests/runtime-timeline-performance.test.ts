import { expect, test } from 'bun:test'
import { groupRuntimeRecords, groupAssistantContinuations } from '../src/features/chat/runtime-timeline-performance'
import type { ConversationMessage } from '../src/features/conversations/model/types'

const message = (id: string, role: 'user' | 'assistant', runId: string | null, content = '') => ({
  id, role, runId, content, conversationId: 'test', kind: 'text', status: 'completed',
  ordinal: 1, createdAt: 100, updatedAt: 100,
}) as ConversationMessage

test('one run keeps one process row and stable identity through model continuations', () => {
  const user = message('user', 'user', null, 'analyze')
  const placeholder = groupAssistantContinuations([user, message('pending', 'assistant', 'run')])
  const messages = [user, ...Array.from({length: 8}, (_, i) => message(`a${i}`, 'assistant', 'run', i % 2 ? `part ${i}` : ''))]
  const grouped = groupAssistantContinuations(messages, 'a7', 'streaming final')
  expect(grouped).toHaveLength(2)
  expect(grouped[1].timelineKey).toBe(placeholder[1].timelineKey)
  expect(grouped[1].id).toBe('a7')
  expect(grouped[1].content).toBe('part 1\n\npart 3\n\npart 5\n\nstreaming final')
  expect(messages[8].content).toBe('part 7')
  expect(groupAssistantContinuations([...messages, message('a8', 'assistant', 'run')])[1].timelineKey).toBe(grouped[1].timelineKey)
})

test('keeps separate runs, legacy messages and user turns distinct', () => {
  const grouped = groupAssistantContinuations([
    message('a1', 'assistant', 'one'), message('a2', 'assistant', 'two'),
    message('u', 'user', null), message('a3', 'assistant', 'two'),
    message('legacy1', 'assistant', null), message('legacy2', 'assistant', null),
  ])
  expect(grouped).toHaveLength(6)
  expect(new Set(grouped.map(m => m.timelineKey)).size).toBe(6)
})

test('retains unchanged run buckets for streaming row memoization', () => {
  const firstEvent = { runId: 'run-a', seq: 1 }
  const otherRunEvent = { runId: 'run-b', seq: 1 }
  const first = groupRuntimeRecords([firstEvent, otherRunEvent], new Map())
  const firstRunBucket = first.get('run-a')
  const otherRunBucket = first.get('run-b')

  const secondEvent = { runId: 'run-a', seq: 2 }
  const second = groupRuntimeRecords([firstEvent, secondEvent, otherRunEvent], first)

  expect(second.get('run-a')).not.toBe(firstRunBucket)
  expect(second.get('run-b')).toBe(otherRunBucket)
  expect(groupRuntimeRecords([firstEvent, secondEvent, otherRunEvent], second).get('run-b')).toBe(otherRunBucket)
})
