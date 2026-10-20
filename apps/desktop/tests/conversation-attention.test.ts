import { describe, expect, test } from 'bun:test'
import {
  EMPTY_CONVERSATION_RUN_OVERLAY,
  applyApprovalToOverlay,
  conversationAttentionLabel,
  conversationRunIndicators,
} from '../src/features/chat/conversation-run-indicator'
import { approvalStatusLabel, toolCallApprovalNote } from '../src/features/conversations/model/approval-labels'

const conversation = (id: string, overrides: Record<string, unknown> = {}) => ({
  id, activeRunId: null, activeRunStatus: null, awaitingApproval: false, ...overrides,
}) as never

describe('"等你处理" marker in the conversation list', () => {
  test('a conversation with a pending approval reads 等你处理; others have no marker', () => {
    const list = [
      conversation('waiting', { activeRunId: 'r1', activeRunStatus: 'running', awaitingApproval: true }),
      conversation('running', { activeRunId: 'r2', activeRunStatus: 'running' }),
      conversation('idle'),
    ]
    const indicators = conversationRunIndicators(list, EMPTY_CONVERSATION_RUN_OVERLAY, 0)
    expect(conversationAttentionLabel(indicators.get('waiting'))).toBe('等你处理')
    expect(conversationAttentionLabel(indicators.get('running'))).toBeNull()
    expect(conversationAttentionLabel(indicators.get('idle'))).toBeNull()
  })

  test('a live approval-requested event marks the row and the resolution clears it', () => {
    const list = [conversation('c1', { activeRunId: 'r1', activeRunStatus: 'running' })]
    const requested = applyApprovalToOverlay(EMPTY_CONVERSATION_RUN_OVERLAY, { conversationId: 'c1', status: 'pending', runId: 'r1' }, 10)
    expect(conversationAttentionLabel(conversationRunIndicators(list, requested, 5).get('c1'))).toBe('等你处理')
    // Hours later nothing expires: still waiting for the user.
    expect(conversationAttentionLabel(conversationRunIndicators(list, requested, 5).get('c1'))).toBe('等你处理')
    const resolved = applyApprovalToOverlay(requested, { conversationId: 'c1', status: 'approved', runId: 'r1' }, 20)
    expect(conversationAttentionLabel(conversationRunIndicators(list, resolved, 5).get('c1'))).toBeNull()
  })
})

describe('approval wording', () => {
  test('an old expired approval reads 已过期, not 用户拒绝', () => {
    expect(approvalStatusLabel('expired')).toBe('已过期')
    expect(approvalStatusLabel('denied')).toBe('用户拒绝')
    const tool = { id: 't1', errorMessage: '用户拒绝' }
    expect(toolCallApprovalNote(tool, [{ toolCallId: 't1', status: 'expired' }])).toBe('已过期')
    expect(toolCallApprovalNote(tool, [{ toolCallId: 't1', status: 'cancelled' }])).toBe('已取消')
    expect(toolCallApprovalNote(tool, [{ toolCallId: 't1', status: 'denied' }])).toBe('用户拒绝')
    expect(toolCallApprovalNote({ id: 't2', errorMessage: null }, [])).toBeNull()
    expect(toolCallApprovalNote(tool, [{ toolCallId: 't1', status: 'expired', decision: { reason: 'approval_wait_timeout' } }])).toBe('已过期')
  })

  test('an approval closed with its conversation or turn reads as ended, not expired', () => {
    const tool = { id: 't1', errorMessage: null }
    expect(toolCallApprovalNote(tool, [{ toolCallId: 't1', status: 'expired', decision: { reason: 'run_interrupted' } }])).toBe('已结束（本轮已结束）')
  })
})
