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

test('saved partial text survives a cancelled reload and later previews cannot overwrite it', () => {
  const live = applyKernelModelPreview(detail(), preview)!
  const cancelled = detail()
  cancelled.kernelSnapshot = { ...cancelled.kernelSnapshot!, state: 'cancelled', terminalWritten: true, lastEventSeq: '8' }
  cancelled.lastRun = { ...cancelled.lastRun!, status: 'cancelled', lastSeq: 8 }
  cancelled.messages = [{ ...live.messages[0], status: 'interrupted', kernelPreview: undefined }]
  const reloaded = mergeConversationDetail(cancelled, live)
  expect(reloaded.messages).toHaveLength(1)
  expect(reloaded.messages[0].content).toBe(preview.text)
  expect(reloaded.messages[0].status).toBe('interrupted')
  expect(applyKernelModelPreview(reloaded, { ...preview, revision: 2, text: 'late' })).toBe(reloaded)
})

test('a completed Kernel response replaces a longer disk-backed streaming draft', () => {
  const live = applyKernelModelPreview(detail(), preview)!
  live.messages[0].kernelPreview = undefined
  const saved = detail()
  saved.messages = [{ ...live.messages[0], content: '短答案', status: 'completed' }]
  expect(mergeConversationDetail(saved, live).messages[0].content).toBe('短答案')
})

test('a new streaming frame replaces the persisted draft instead of adding the same message twice', () => {
  const live = applyKernelModelPreview(detail(), preview)!
  live.messages[0].kernelPreview = undefined
  const updated = applyKernelModelPreview(live, { ...preview, revision: 2, text: '继续输出' })!
  expect(updated.messages).toHaveLength(1)
  expect(updated.messages[0].content).toBe('继续输出')
})


test('shorter Kernel previews and reasoning survive a same-checkpoint reload without mixed revisions', () => {
  const stored = applyKernelModelPreview(detail(), { ...preview, text: '很长的旧草稿', reasoning: '旧思考' }, 10)!
  stored.messages[0].kernelPreview = undefined
  const live = applyKernelModelPreview(stored, { ...preview, revision: 2, text: '短答', reasoning: '新思考' }, 11)!
  const merged = mergeConversationDetail(stored, live)
  expect(merged.messages[0].content).toBe('短答')
  expect(merged.messages[0].kernelPreview?.reasoning).toBe('新思考')
  expect(merged.messages[0].kernelPreview?.revision).toBe(2)
})

test('a loaded run removes a superseded Kernel row but retains previously paged runs', () => {
  const current = applyKernelModelPreview(detail(), preview)!
  current.messages[0].kernelPreview = undefined
  current.messages[0].status = 'completed'
  const persisted = detail()
  persisted.messages = [{ ...current.messages[0], id: 'user-run', role: 'user', content: 'question' }]
  const old = { ...current.messages[0], id: 'kernel-message:older:2', runId: 'older' }
  current.messages.push(old)
  const merged = mergeConversationDetail(persisted, current)
  expect(merged.messages.map(message => message.id)).toEqual(['user-run', old.id])
})
