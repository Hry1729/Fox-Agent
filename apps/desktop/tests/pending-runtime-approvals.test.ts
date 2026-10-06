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
