import { expect, test } from 'bun:test'
import { groupCompanionCommunications } from '../src/features/companions/assignment-timeline-model'
import { coordinationTranscriptEntries } from '../src/features/companions/coordination-transcript'
import type { CompanionAssignment, ConversationDetail } from '../src/features/conversations/model/types'

const assignment = (index: number, patch: Partial<CompanionAssignment> = {}): CompanionAssignment => ({
  id: `a-${index}`, rootRunId: 'root', sourceRunId: `source-${index}`, sourceConversationId: 'chat', sourceCompanionId: 'chief',
  sourceName: '小狐', recipientConversationId: 'pair', recipientCompanionId: 'peer', recipientName: '人话',
  title: '报数', instructions: String(index * 2 + 1), result: String(index * 2 + 2), status: 'completed', deliveryStatus: 'delivered', reply: 'resume',
  createdAt: index * 10 + 2, updatedAt: index * 10 + 3, ...patch,
})

test('同一任务的补答保留全部往来，主聊天计数与详情气泡一致', () => {
  const record = assignment(0, { messages: [
    { id: 'sent', direction: 'sent', text: '查数据', createdAt: 2 },
    { id: 'question', direction: 'received', text: '哪个月份？', createdAt: 3 },
    { id: 'answer', direction: 'sent', text: '上个月', createdAt: 4 },
    { id: 'result', direction: 'received', text: '已查到', createdAt: 5 },
  ] })
  expect(groupCompanionCommunications([record], [], 'chat')[0].events).toHaveLength(4)
  const detail = { conversation: { id: 'pair' }, messages: [], runtimeEvents: [], runs: [], toolCalls: [], approvals: [], attachments: [], artifacts: [] } as unknown as ConversationDetail
  expect(coordinationTranscriptEntries([record], detail).map(entry => entry.text)).toEqual(['查数据', '哪个月份？', '上个月', '已查到'])
})

test('没有汇报穿插时按双向消息数合并，用户消息和汇报都打断分组', () => {
  const records = [0, 1, 2, 3, 4].map(index => assignment(index))
  const groups = groupCompanionCommunications(records, [
    { id: 'one', createdAt: 1 }, { id: 'three', createdAt: 4 }, { id: 'user-update', createdAt: 5 },
    { id: 'ack', createdAt: 6 }, { id: 'done', createdAt: 50 },
  ], 'chat', 'chief')
  expect(groups.map(group => [group.beforeId, group.events.length])).toEqual([['three', 2], ['done', 8]])
  expect(groups[1].events.map(event => event.direction)).toEqual(['sent', 'received', 'sent', 'received', 'sent', 'received', 'sent', 'received'])
  const interrupted = groupCompanionCommunications([assignment(0)], [{ id: 'report', createdAt: 2.5 }], 'chat', 'chief')
  expect(interrupted.map(group => [group.beforeId, group.events.length])).toEqual([['report', 1], [null, 1]])
})

test('不同伙伴不合并，接收方方向翻转，排队与失败不伪装成消息往来', () => {
  const records = [assignment(0), assignment(1, { recipientCompanionId: 'other', recipientName: '审校', recipientConversationId: 'other-pair' }),
    assignment(2, { status: 'queued', deliveryStatus: 'queued', result: null }), assignment(3, { status: 'failed', deliveryStatus: 'failed', result: '投递失败' })]
  const groups = groupCompanionCommunications(records, [], 'chat')
  expect(groups.map(group => group.events.filter(event => event.delivered).length)).toEqual([2, 2, 0, 0])
  expect(groupCompanionCommunications([assignment(0)], [], 'peer-private', 'peer')[0].events.map(event => event.direction)).toEqual(['received', 'sent'])
  expect(groupCompanionCommunications([assignment(0, { reply: 'none' })], [], 'chat')[0].events).toHaveLength(1)
})

test('回传标识在唤醒后的汇报之前，加载更早消息不会重复计算旧段', () => {
  const record = assignment(0, { updatedAt: 30, resumeRunId: 'resume' })
  const groups = groupCompanionCommunications([record], [{ id: 'report', createdAt: 20, runId: 'resume' }], 'chat')
  expect(groups[0].beforeId).toBe('report')
  expect(groups[0].events).toHaveLength(2)
  expect(groupCompanionCommunications([record], [], 'chat', undefined, 31)).toEqual([])
})

test('协作气泡显示真实派发正文与持久回复，不重复回传，也不泄露内部启动提示', () => {
  const detail = {
    conversation: { id: 'pair' }, runtimeEvents: [], attachments: [], artifacts: [], toolCalls: [], approvals: [], lastRun: null, runs: [],
    messages: [
      { id: 'internal', runId: 'recipient-run', conversationId: 'pair', role: 'system', kind: 'internal', content: '处理伙伴派来的协作事项', status: 'completed', createdAt: 2, updatedAt: 2, ordinal: 1 },
      { id: 'reply', runId: 'recipient-run', conversationId: 'pair', role: 'assistant', kind: 'text', content: '2', status: 'completed', createdAt: 3, updatedAt: 3, ordinal: 2 },
    ],
  } as unknown as ConversationDetail
  const entries = coordinationTranscriptEntries([assignment(0, { recipientRunId: 'recipient-run', instructions: '我报1，请你报2' })], detail)
  expect(entries.map(entry => [entry.author, entry.text])).toEqual([['source', '我报1，请你报2'], ['recipient', '2']])
})
