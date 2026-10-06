import { describe, expect, test } from 'bun:test'
import {
  EMPTY_CONVERSATION_RUN_OVERLAY,
  applyApprovalToOverlay,
  applyRunEventToOverlay,
  conversationRunIndicator,
  conversationRunIndicators,
  pruneRunOverlay,
  type ConversationRunOverlay,
} from '../src/features/chat/conversation-run-indicator'

type Summary = Parameters<typeof conversationRunIndicator>[0]
const summary = (overrides: Partial<Summary> = {}): Summary => ({
  activeRunId: null,
  activeRunStatus: null,
  awaitingApproval: false,
  ...overrides,
})

const conversation = (id: string, overrides: Partial<Summary> = {}) => ({ id, ...summary(overrides) })

const event = (conversationId: string, type: string, runId = `run-${conversationId}`) => ({ conversationId, runId, event: { type } })

describe('conversation run indicator from persisted state', () => {
  test('an idle conversation has no indicator', () => {
    expect(conversationRunIndicator(summary())).toBeNull()
    expect(conversationRunIndicator(summary({ activeRunStatus: 'running' }))).toBeNull()
  })

  test('a queued run reads as queued and a started run as running', () => {
    expect(conversationRunIndicator(summary({ activeRunId: 'run-1', activeRunStatus: 'queued' }))).toBe('queued')
    expect(conversationRunIndicator(summary({ activeRunId: 'run-1', activeRunStatus: 'running' }))).toBe('running')
    expect(conversationRunIndicator(summary({ activeRunId: 'run-1', activeRunStatus: 'cancelling' }))).toBe('running')
  })

  test('a pending approval outranks a plain running run', () => {
    expect(conversationRunIndicator(summary({ activeRunId: 'run-1', activeRunStatus: 'running', awaitingApproval: true }))).toBe('approval')
    expect(conversationRunIndicator(summary({ activeRunId: 'run-1', activeRunStatus: 'awaiting_confirmation' }))).toBe('approval')
  })
})

describe('live overlay', () => {
  test('a run starting in another conversation marks only that conversation', () => {
    let overlay: ConversationRunOverlay = EMPTY_CONVERSATION_RUN_OVERLAY
    overlay = applyRunEventToOverlay(overlay, event('conversation-a', 'run.started'), 100)
    overlay = applyRunEventToOverlay(overlay, event('conversation-a', 'reasoning.delta'), 120)
    overlay = applyRunEventToOverlay(overlay, event('conversation-b', 'run.started'), 130)
    expect(overlay.get('conversation-a')?.indicator).toBe('running')
    expect(overlay.get('conversation-b')?.indicator).toBe('running')
    expect(overlay.size).toBe(2)
  })

  test('repeat deltas keep the same map, so a long run never re-renders the sidebar', () => {
    let overlay = applyRunEventToOverlay(EMPTY_CONVERSATION_RUN_OVERLAY, event('conversation-a', 'run.started'), 100)
    const before = overlay
    overlay = applyRunEventToOverlay(overlay, event('conversation-a', 'message.started'), 110)
    overlay = applyRunEventToOverlay(overlay, event('conversation-a', 'reasoning.delta'), 120)
    expect(overlay).toBe(before)
  })

  test('a terminal event stops only its own conversation', () => {
    let overlay: ConversationRunOverlay = EMPTY_CONVERSATION_RUN_OVERLAY
    overlay = applyRunEventToOverlay(overlay, event('conversation-a', 'run.started'), 100)
    overlay = applyRunEventToOverlay(overlay, event('conversation-b', 'run.started'), 110)
    overlay = applyRunEventToOverlay(overlay, event('conversation-a', 'run.completed'), 200)
    expect(overlay.get('conversation-a')?.indicator).toBe('completed')
    expect(overlay.get('conversation-b')?.indicator).toBe('running')
    for (const terminal of ['run.cancelled', 'run.failed', 'run.interrupted']) {
      const next = applyRunEventToOverlay(overlay, event('conversation-b', terminal), 250)
      expect(next.get('conversation-b')?.indicator).toBeNull()
    }
  })

  test('waiting for approval survives the unrelated events a waiting run still emits', () => {
    let overlay = applyRunEventToOverlay(EMPTY_CONVERSATION_RUN_OVERLAY, event('conversation-a', 'run.started'), 100)
    overlay = applyRunEventToOverlay(overlay, event('conversation-a', 'run.waiting_approval'), 150)
    expect(overlay.get('conversation-a')?.indicator).toBe('approval')
    overlay = applyRunEventToOverlay(overlay, event('conversation-a', 'usage.updated'), 160)
    expect(overlay.get('conversation-a')?.indicator).toBe('approval')
  })

  test('malformed notifications never create or move an indicator', () => {
    const started = applyRunEventToOverlay(EMPTY_CONVERSATION_RUN_OVERLAY, event('conversation-a', 'run.started'), 100)
    expect(applyRunEventToOverlay(started, { conversationId: '', runId: 'run-a', event: { type: 'run.completed' } }, 200)).toBe(started)
    expect(applyRunEventToOverlay(started, { conversationId: 'conversation-a', runId: 'run-a', event: {} }, 200)).toBe(started)
  })

  test('a pending approval marks the conversation and resolving it clears just the marker', () => {
    let overlay: ConversationRunOverlay = EMPTY_CONVERSATION_RUN_OVERLAY
    overlay = applyRunEventToOverlay(overlay, event('conversation-a', 'run.started'), 100)
    overlay = applyApprovalToOverlay(overlay, { conversationId: 'conversation-a', status: 'pending' }, 120)
    expect(overlay.get('conversation-a')?.indicator).toBe('approval')
    // Repeating the same pending approval changes nothing.
    expect(applyApprovalToOverlay(overlay, { conversationId: 'conversation-a', status: 'pending' }, 130)).toBe(overlay)
    overlay = applyApprovalToOverlay(overlay, { conversationId: 'conversation-a', status: 'approved' }, 140)
    expect(overlay.has('conversation-a')).toBe(false)
  })
})

describe('reconciling live evidence with the authoritative list', () => {
  test('live evidence newer than the snapshot wins, older evidence loses', () => {
    const conversations = [conversation('conversation-a'), conversation('conversation-b', { activeRunId: 'run-b', activeRunStatus: 'queued' })]
    const overlay: ConversationRunOverlay = new Map([
      ['conversation-a', { indicator: 'running', updatedAt: 150 }],
      ['conversation-b', { indicator: null, updatedAt: 90 }],
    ])
    const indicators = conversationRunIndicators(conversations, overlay, 100)
    // A started after the snapshot: still running even though the snapshot is idle.
    expect(indicators.get('conversation-a')).toBe('running')
    // B's tombstone is older than the snapshot, so the authoritative queued state stands.
    expect(indicators.get('conversation-b')).toBe('queued')
  })

  test('a terminal tombstone hides a snapshot that predates the terminal event', () => {
    const conversations = [conversation('conversation-a', { activeRunId: 'run-a', activeRunStatus: 'running' })]
    const overlay: ConversationRunOverlay = new Map([['conversation-a', { indicator: null, updatedAt: 200 }]])
    expect(conversationRunIndicators(conversations, overlay, 100).has('conversation-a')).toBe(false)
  })

  test('pruning keeps entries the newest snapshot cannot account for', () => {
    const overlay: ConversationRunOverlay = new Map([
      ['gone', { indicator: 'running', updatedAt: 150 }],
      ['fresh', { indicator: 'running', updatedAt: 150 }],
      ['superseded', { indicator: 'running', updatedAt: 90 }],
      ['ended', { indicator: null, updatedAt: 90 }],
    ])
    const pruned = pruneRunOverlay(overlay, [conversation('fresh'), conversation('superseded'), conversation('ended')], 100)
    expect([...pruned.keys()].sort()).toEqual(['fresh'])
  })

  test('pruning an already-clean overlay keeps the same map', () => {
    const overlay: ConversationRunOverlay = new Map([['fresh', { indicator: 'running', updatedAt: 150 }]])
    expect(pruneRunOverlay(overlay, [conversation('fresh')], 100)).toBe(overlay)
  })

  test('a conversation that leaves the list loses its indicator', () => {
    const overlay: ConversationRunOverlay = new Map([['conversation-a', { indicator: 'running', updatedAt: 150 }]])
    expect(conversationRunIndicators([conversation('conversation-b')], overlay, 100).size).toBe(0)
  })

  test('a stale post-terminal list cannot revive the same queued run', () => {
    const listed = [conversation('conversation-a', { activeRunId: 'run-a', activeRunStatus: 'queued' })]
    const overlay = applyRunEventToOverlay(EMPTY_CONVERSATION_RUN_OVERLAY, event('conversation-a', 'run.completed', 'run-a'), 200)
    const pruned = pruneRunOverlay(overlay, listed, 250)
    expect(pruned.get('conversation-a')?.indicator).toBe('completed')
    expect(conversationRunIndicators(listed, pruned, 250).get('conversation-a')).toBe('completed')

    const settled = [conversation('conversation-a')]
    expect(pruneRunOverlay(pruned, settled, 300).has('conversation-a')).toBe(false)
  })

  test('an old terminal event cannot hide a new run', () => {
    const nextRun = [conversation('conversation-a', { activeRunId: 'run-b', activeRunStatus: 'queued' })]
    const overlay = applyRunEventToOverlay(EMPTY_CONVERSATION_RUN_OVERLAY, event('conversation-a', 'run.completed', 'run-a'), 200)
    expect(conversationRunIndicators(nextRun, overlay, 100).get('conversation-a')).toBe('queued')
    expect(pruneRunOverlay(overlay, nextRun, 100).has('conversation-a')).toBe(false)
  })

  test('a new run event replaces the terminal marker even when it is also running', () => {
    let overlay = applyRunEventToOverlay(EMPTY_CONVERSATION_RUN_OVERLAY, event('conversation-a', 'run.completed', 'run-a'), 200)
    overlay = applyRunEventToOverlay(overlay, event('conversation-a', 'run.started', 'run-b'), 210)
    expect(overlay.get('conversation-a')).toMatchObject({ indicator: 'running', runId: 'run-b' })
  })
})


describe('four approved icon states', () => {
  test('a successful completion survives a list refresh while failures do not become green checks', () => {
    expect(conversationRunIndicator(summary({ lastRunId: 'done', lastRunStatus: 'completed' }))).toBe('completed')
    for (const status of ['failed', 'cancelled', 'interrupted']) {
      expect(conversationRunIndicator(summary({ lastRunId: 'ended', lastRunStatus: status }))).toBeNull()
    }
  })
  test('a persisted pending choice outranks completion', () => {
    expect(conversationRunIndicator(summary({ lastRunId: 'question', lastRunStatus: 'completed', awaitingReply: true }))).toBe('reply')
  })
  test('question completion remains waiting until the user responds', () => {
    let overlay = applyRunEventToOverlay(EMPTY_CONVERSATION_RUN_OVERLAY, event('a', 'user.question.requested', 'run-a'), 10)
    overlay = applyRunEventToOverlay(overlay, { ...event('a', 'run.completed', 'run-a'), event: { type: 'run.completed', completionReason: 'awaiting_user' } }, 20)
    expect(overlay.get('a')?.indicator).toBe('reply')
    overlay = applyRunEventToOverlay(overlay, event('a', 'usage.updated', 'run-a'), 30)
    expect(overlay.get('a')?.indicator).toBe('reply')
    overlay = applyRunEventToOverlay(overlay, event('a', 'user.question.responded', 'run-a'), 40)
    expect(overlay.get('a')?.indicator).toBe('running')
  })
  test('late usage events cannot turn a completed conversation back into running', () => {
    const ended = applyRunEventToOverlay(EMPTY_CONVERSATION_RUN_OVERLAY, event('a', 'run.completed', 'run-a'), 10)
    expect(applyRunEventToOverlay(ended, event('a', 'usage.updated', 'run-a'), 20)).toBe(ended)
  })
})


test('awaiting-user completion is waiting even if the question notification arrived late', () => {
  const notification = { ...event('a', 'run.completed', 'run-a'), event: { type: 'run.completed', completionReason: 'awaiting_user' } }
  expect(applyRunEventToOverlay(EMPTY_CONVERSATION_RUN_OVERLAY, notification, 10).get('a')?.indicator).toBe('reply')
})


test('ordinary completion closes a question instead of leaving the row waiting', () => {
  const waiting = applyRunEventToOverlay(EMPTY_CONVERSATION_RUN_OVERLAY, event('a', 'user.question.requested', 'run-a'), 10)
  const completed = applyRunEventToOverlay(waiting, event('a', 'run.completed', 'run-a'), 20)
  expect(completed.get('a')?.indicator).toBe('completed')
})
