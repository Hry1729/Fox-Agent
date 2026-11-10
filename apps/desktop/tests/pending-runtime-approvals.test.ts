import { expect, test } from 'bun:test'
import { pendingRuntimeApprovals } from '../src/features/conversations/model/pending-interactions'
import type { ApprovalRecord, ConversationDetail } from '../src/features/conversations/model/types'

const approval = {id:'ticket',toolCallId:'tool-1',runId:'run-1',conversationId:'a',status:'pending',request:{},requestedAt:1} as ApprovalRecord
const detail = (status:string,toolStatus='pending') => ({approvals:[approval],lastRun:{id:'run-1',status},runs:[],childRuns:[],toolCalls:[{id:'tool-1',runId:'run-1',status:toolStatus}]} as unknown as ConversationDetail)

test('completed O12-style work cannot keep a pending card or lock the composer', () => {
  expect(pendingRuntimeApprovals(detail('completed','completed'))).toEqual([])
  expect(pendingRuntimeApprovals(detail('running','completed'))).toEqual([])
  for(const state of ['failed','cancelled','interrupted']) expect(pendingRuntimeApprovals(detail(state))).toEqual([])
})

test('a completed parent does not remove a still-running child approval', () => {
  const value=detail('completed')
  value.approvals=[{...approval,runId:'child',toolCallId:'child-tool'}]
  value.childRuns=[{childRunId:'child',status:'running'} as any]
  expect(pendingRuntimeApprovals(value)).toEqual(value.approvals)
  value.childRuns[0].status='completed'
  expect(pendingRuntimeApprovals(value)).toEqual([])
})

test('valid approvals remain available while their owning work waits', () => {
  expect(pendingRuntimeApprovals(detail('running'))).toEqual([approval])
})

test('invalidated_approval_does_not_block_composer', () => {
  // D1-6: even in the registration→settlement window (status still pending, run
  // and tool still running) a persistent invalidation must not raise the old
  // interaction or lock the composer.
  const invalidated = { ...approval, decision: { scope: 'once', invalidation: { code: 'companion.tool_not_available', message: '联系人已撤销对工具的授权', at: 2 } } } as ApprovalRecord
  const value = detail('running')
  value.approvals = [invalidated]
  expect(pendingRuntimeApprovals(value)).toEqual([])
  // A malformed barrier is not an invalidation; the pending card stays.
  const corrupt = { ...approval, decision: { invalidation: { code: '', message: '', at: 0 } } } as ApprovalRecord
  value.approvals = [corrupt]
  expect(pendingRuntimeApprovals(value)).toEqual([corrupt])
  // A legitimate pending approval without a barrier still blocks.
  value.approvals = [approval]
  expect(pendingRuntimeApprovals(value)).toEqual([approval])
})
