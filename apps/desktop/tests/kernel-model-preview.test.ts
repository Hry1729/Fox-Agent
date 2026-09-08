import { expect, test } from 'bun:test'
import { applyKernelModelPreview } from '../src/features/conversations/model/kernel-model-preview'
import { mergeConversationDetail } from '../src/features/conversations/model/runtime-event-reducer'
import type { ConversationDetail } from '../src/features/conversations/model/types'

function detail(): ConversationDetail {
  return { conversation: { id: 'conversation' }, lastRun: { id: 'run', status: 'running', lastSeq: 3 },
    kernelSnapshot: { schemaVersion: 1, runId: 'run', turnId: 'turn', state: 'running', lastEventSeq: '3', terminalWritten: false },
    messages: [], runtimeEvents: [], toolCalls: [], approvals: [], attachments: [], artifacts: [], knowledgeBindings: [], goals: [], tasks: [], evidence: [],
  } as unknown as ConversationDetail
}
const preview = { schemaVersion: 1, runId: 'run', conversationId: 'conversation', turnId: 'turn', checkpointSeq: 2, revision: 1, text: '正在回答 中文 😀' }

test('streaming previews replace text without advancing facts or accepting duplicates', () => {
  const original = detail()
  const first = applyKernelModelPreview(original, preview, 10)!
  expect(first.messages[0].status).toBe('streaming')
  expect(first.kernelSnapshot).toBe(original.kernelSnapshot)
  expect(first.lastRun).toBe(original.lastRun)
  expect(applyKernelModelPreview(first, preview)).toBe(first)
  const next = applyKernelModelPreview(first, { ...preview, revision: 2, text: '更正后' }, 11)!
  expect(next.messages).toHaveLength(1)
  expect(next.messages[0].content).toBe('更正后')
  expect(next.messages[0].createdAt).toBe(10)
  expect(original.messages).toHaveLength(0)
})

test('foreign cursors, excessive text and non-display fields cannot enter chat', () => {
  const current = detail()
  for (const patch of [{ schemaVersion: 2 }, { runId: 'other' }, { conversationId: 'other' }, { turnId: 'other' },
    { checkpointSeq: 4 }, { revision: 0 }, { revision: 1.5 }, { thinking: 'not display' }, { text: '中'.repeat(90_000) }]) {
    expect(applyKernelModelPreview(current, { ...preview, ...patch })).toBe(current)
  }
  current.kernelSnapshot!.state = 'cancelling'
  expect(applyKernelModelPreview(current, preview)).toBe(current)
})

test('persisted final text wins even when shorter; cancellation and later rounds discard drafts', () => {
  const current = applyKernelModelPreview(detail(), preview)!
  const stored = detail()
  stored.kernelSnapshot = { ...stored.kernelSnapshot!, lastEventSeq: '5', state: 'completed', terminalWritten: true }
  stored.lastRun = { ...stored.lastRun!, lastSeq: 5, status: 'completed' }
  stored.messages = [{ ...current.messages[0], id: 'kernel-message:run:2', status: 'completed', content: '最终回复', kernelPreview: undefined }]
  const final = mergeConversationDetail(stored, current)
  expect(final.messages.map(message => message.content)).toEqual(['最终回复'])
  expect(applyKernelModelPreview(final, { ...preview, revision: 5 })).toBe(final)
  for (const state of ['cancelled', 'running']) {
    const next = detail()
    next.kernelSnapshot = { ...next.kernelSnapshot!, lastEventSeq: '8', state, terminalWritten: state === 'cancelled' }
    next.lastRun = { ...next.lastRun!, lastSeq: 8, status: state }
    expect(mergeConversationDetail(next, current).messages).toHaveLength(0)
  }
  expect(mergeConversationDetail(detail(), current).messages).toHaveLength(1)
})
