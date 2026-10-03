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

const event = (conversationId: string, type: string) => ({ conversationId, event: { type } })

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
    expect(overlay.get('conversation-a')?.indicator).toBeNull()
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
    expect(applyRunEventToOverlay(started, { conversationId: '', event: { type: 'run.completed' } }, 200)).toBe(started)
    expect(applyRunEventToOverlay(started, { conversationId: 'conversation-a', event: {} }, 200)).toBe(started)
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
    const pruned = pruneRunOverlay(overlay, new Set(['fresh', 'superseded', 'ended']), 100)
    expect([...pruned.keys()].sort()).toEqual(['fresh'])
  })

  test('pruning an already-clean overlay keeps the same map', () => {
    const overlay: ConversationRunOverlay = new Map([['fresh', { indicator: 'running', updatedAt: 150 }]])
    expect(pruneRunOverlay(overlay, new Set(['fresh']), 100)).toBe(overlay)
  })

  test('a conversation that leaves the list loses its indicator', () => {
    const overlay: ConversationRunOverlay = new Map([['conversation-a', { indicator: 'running', updatedAt: 150 }]])
    expect(conversationRunIndicators([conversation('conversation-b')], overlay, 100).size).toBe(0)
  })
})
