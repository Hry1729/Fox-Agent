import { describe, expect, test } from 'bun:test'
import { conversationRunFingerprint, conversationRunIsActive, conversationRunState, mergeKernelSnapshot, snapshotForRun } from '../src/features/conversations/model/kernel-snapshot'
import type { ConversationDetail, KernelRunSnapshot } from '../src/features/conversations/model/types'

function detail(seq: string, patch: Partial<KernelRunSnapshot> = {}): ConversationDetail {
  return {
    lastRun: { id: 'run-1', lastSeq: 999999 },
    messages: [],
    kernelSnapshot: {
      schemaVersion: 1, runId: 'run-1', turnId: 'turn-1', engineId: 'pi', state: 'running',
      lastEventSeq: seq, terminalWritten: false, runningElapsedMs: 0, tools: [],
      providerAttempts: 0, turnAttempts: 0, compactions: 0, ...patch,
    },
  } as ConversationDetail
}

describe('authoritative snapshot read model', () => {
  test('drives polling and liveness from authority while preserving Legacy behavior', () => {
    const waiting = detail('12', { state: 'waiting_approval' })
    waiting.lastRun!.status = 'completed'
    expect(conversationRunState(waiting)).toBe('waiting_approval')
    expect(conversationRunIsActive(waiting)).toBe(true)
    expect(conversationRunFingerprint(waiting)).toBe('kernel:waiting_approval:12:0')
    const completed = detail('13', { state: 'completed', terminalWritten: true })
    completed.lastRun!.status = 'running'
    expect(conversationRunIsActive(completed)).toBe(false)
    delete completed.kernelSnapshot
    expect(conversationRunIsActive(completed)).toBe(true)
    expect(conversationRunFingerprint(completed)).toBe('legacy:running:999999:0')
  })
  test('does not derive authority from Legacy events or a different run', () => {
    expect(snapshotForRun({ lastRun: { id: 'run-1' } } as ConversationDetail)).toBeNull()
    expect(snapshotForRun(detail('1', { runId: 'other' }))).toBeNull()
    expect(snapshotForRun(detail('1', { schemaVersion: 2 }))).toBeNull()
  })
  test('accepts exactly the Kernel lifecycle and consistent terminal facts', () => {
    for (const state of ['created', 'running', 'waiting_approval', 'retry_scheduled', 'compacting', 'cancelling']) {
      expect(snapshotForRun(detail('1', { state }))?.state).toBe(state)
    }
    for (const state of ['completed', 'cancelled', 'failed', 'budget_exhausted', 'approval_expired']) {
      expect(snapshotForRun(detail('1', { state, terminalWritten: true }))?.state).toBe(state)
      expect(snapshotForRun(detail('1', { state }))).toBeNull()
    }
    expect(snapshotForRun(detail('1', { state: 'unknown' }))).toBeNull()
  })
  test('merges only Kernel revisions without JavaScript precision loss', () => {
    const older = detail('9007199254740992')
    const newer = detail('9007199254740993', { state: 'completed', terminalWritten: true })
    expect(mergeKernelSnapshot(older, newer)).toBe(newer.kernelSnapshot)
    expect(mergeKernelSnapshot(newer, older)).toBe(newer.kernelSnapshot)
    expect(mergeKernelSnapshot(detail('2'), detail('1'))?.lastEventSeq).toBe('2')
  })
  test('rejects malformed sequences and does not carry authority to another run', () => {
    for (const seq of ['-1', '1.5', '1e3', '01', '']) expect(snapshotForRun(detail(seq))).toBeNull()
    const next = detail('2', { runId: 'run-2' })
    next.lastRun!.id = 'run-2'
    expect(mergeKernelSnapshot(next, detail('999'))?.runId).toBe('run-2')
  })
})
