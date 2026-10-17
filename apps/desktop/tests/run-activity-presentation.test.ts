import { expect, test } from 'bun:test'
import { presentConversationActivity, presentRunActivity, summarizeRunSteps } from '../src/features/chat/run-activity-presentation'
import { applyKernelModelPreview } from '../src/features/conversations/model/kernel-model-preview'
import type { ConversationDetail, RunEventRecord } from '../src/features/conversations/model/types'

const event = (seq: number, eventType: string, data: Record<string, unknown> = {}, runId = 'r'): RunEventRecord => ({ runId, seq, eventType, event: { type: eventType, ...data }, createdAt: 1000 + seq * 100 })
const start = event(1, 'run.started')
const present = (events: RunEventRecord[], state = 'running') => presentRunActivity(events, { runId: 'r', state })

test('live tools and progress remain visible while a streaming preview is retained', () => {
  for (const currentPreview of ['answer', 'reasoning'] as const) {
    const events = [start, event(2, 'tool.started', { toolCallId: 't', tool: 'web_search' })]
    expect(presentRunActivity(events, { runId: 'r', currentPreview }).animation).toBe('radar')
    events.push(event(3, 'tool.updated', { toolCallId: 't', progress: 40 }))
    expect(presentRunActivity(events, { runId: 'r', currentPreview })).toMatchObject({ animation: 'progress', progress: 40 })
    events.push(event(4, 'tool.completed', { toolCallId: 't' }))
    expect(presentRunActivity(events, { runId: 'r', currentPreview }).animation).toBe(currentPreview === 'answer' ? 'writing' : 'thinking')
  }
})

test('an unselected run cannot acquire status from another conversation or prose', () => {
  expect(presentRunActivity([start, event(2, 'run.failed')], {}).label).toBe('准备就绪')
  expect(present([event(1, 'run.failed', {}, 'other')]).label).toBe('正在处理')
  expect(present([start, event(2, 'message.delta', { delta: '正在读取 100 个文件，然后运行命令' })]).animation).toBe('writing')
  expect(summarizeRunSteps([start, event(2, 'message.delta', { delta: '已读取 100 个文件' })], true)).toBe('')
})

test.each([
  ['queued', '等待开始', 'humming'], ['created', '等待开始', 'humming'],
  ['waiting_approval', '等待你的确认', 'listening'], ['retry_scheduled', '等待重试', 'humming'],
  ['compacting', '正在整理上下文', 'loading'], ['cancelling', '正在停止', 'loading'],
  ['completed', '已完成', 'happy'], ['failed', '处理失败', 'alerting'],
  ['cancelled', '已停止', 'idle'], ['interrupted', '已中断', 'alerting'],
  ['budget_exhausted', '处理失败', 'alerting'], ['approval_expired', '处理失败', 'alerting'],
])('persisted state %s maps to its own presentation', (state, label, animation) => {
  expect(present([], state)).toMatchObject({ label, animation })
})

test.each([
  ['preparing', '正在准备', 'loading'], ['request_sent', '等待回复', 'receiving'], ['finalizing', '正在收尾', 'loading'],
])('phase %s supersedes old content', (phase, label, animation) => {
  expect(present([start, event(2, 'message.delta'), event(3, 'run.phase', { phase })])).toMatchObject({ label, animation })
})

test.each([
  ['read_file', '正在读取文件', 'searching'], ['list_directory', '正在查看目录', 'searching'],
  ['query_kb', '正在检索资料', 'searching'], ['web_search', '正在搜索网页', 'radar'],
  ['run_command', '正在执行命令', 'orbit'], ['edit_file', '正在修改文件', 'writing'],
  ['call_mcp_tool', '正在调用工具', 'orbit'],
])('live tool %s uses observed execution rather than model text', (tool, label, animation) => {
  expect(present([start, event(2, 'tool.started', { toolCallId: 't', tool }), event(3, 'message.delta')])).toMatchObject({ label, animation })
})

test('thought, tool, response, waiting and terminal events transition without a stale tool', () => {
  const events = [start, event(2, 'reasoning.delta', { delta: '分析' })]
  expect(present(events).animation).toBe('thinking')
  events.push(event(3, 'tool.started', { toolCallId: 't', tool: 'read_file' }))
  expect(present(events).animation).toBe('searching')
  events.push(event(4, 'tool.completed', { toolCallId: 't', isError: true }))
  expect(present(events)).toMatchObject({ active: true, animation: 'loading' })
  events.push(event(5, 'message.delta'))
  expect(present(events).animation).toBe('writing')
  events.push(event(6, 'run.phase', { phase: 'request_sent' }))
  expect(present(events).animation).toBe('receiving')
  events.push(event(7, 'run.completed'), event(8, 'tool.started', { toolCallId: 'late', tool: 'read_file' }))
  expect(present(events)).toMatchObject({ active: false, outcome: 'completed', startedAt: 1100, endedAt: 1700 })
})

test('approval and questions outrank tools, while terminal state closes both', () => {
  const events = [start, event(2, 'tool.started', { toolCallId: 't', tool: 'write_file' })]
  expect(presentRunActivity(events, { runId: 'r', approvalPending: true }).label).toBe('等待你的确认')
  expect(presentRunActivity(events, { runId: 'r', questionPending: true }).label).toBe('等待你的回答')
  expect(presentRunActivity(events, { runId: 'r', state: 'cancelled', approvalPending: true, questionPending: true }).label).toBe('已停止')
})

test('completed and failed compaction do not pretend to be reasoning or terminal run failure', () => {
  expect(present([start, event(2, 'context.compaction.completed')]).label).toBe('正在处理')
  expect(present([start, event(2, 'context.compaction.failed')])).toMatchObject({ label: '上下文整理失败', active: true })
})

test('progress is numeric evidence, never elapsed time or an arbitrary string', () => {
  const events = [start, event(2, 'tool.started', { toolCallId: 't', tool: 'call_mcp_tool' })]
  expect(present([...events, event(3, 'tool.updated', { toolCallId: 't', progress: 42 })])).toMatchObject({ animation: 'progress', progress: 42 })
  for (const progress of ['80%', -1, 101, NaN]) expect(present([...events, event(3, 'tool.updated', { toolCallId: 't', progress })]).animation).toBe('orbit')
})

test('summary counts proven success, distinct file paths and distinct commands; replay cannot inflate it', () => {
  const events = [start]
  const call = (id: string, tool: string, path?: string, isError = false) => {
    events.push(event(events.length + 1, 'tool.started', { toolCallId: id, tool, input: { path } }))
    events.push(event(events.length + 1, 'tool.completed', { toolCallId: id, isError }))
  }
  call('a', 'read_file', 'D:\\Project\\A.txt')
  call('b', 'read_file', 'd:/project/a.txt')
  call('c', 'read_file', 'missing.txt', true)
  call('ls', 'list_directory', 'docs')
  call('cmd', 'run_command')
  call('poll', 'write_stdin')
  events.push(event(events.length + 1, 'tool.started', { toolCallId: 'pending', tool: 'write_file', input: { path: 'new.txt' } }))
  const summary = summarizeRunSteps([...events, ...events].reverse(), true, 'r')
  expect(summary).toBe('已读取 1 个文件 · 已查看 1 个目录 · 已执行 1 次命令 · 完成 1 次工具调用 · 1 次调用失败 · 1 项进行中')
  expect(summarizeRunSteps([...events, event(30, 'run.cancelled')], false)).toContain('1 项未完成')
  expect(summarizeRunSteps(events, true, 'other')).toBe('')
})

test('missing path or call identity cannot invent file counts', () => {
  expect(summarizeRunSteps([event(1, 'tool.completed', { toolCallId: 'a', tool: 'read_file' })], false)).toBe('完成 1 次读取')
  expect(summarizeRunSteps([event(1, 'tool.completed', { tool: 'read_file', input: { path: 'x' } })], false)).toBe('')
})

test('Kernel streaming previews are displayed only while their checkpoint owns the current snapshot', () => {
  const detail = { conversation: { id: 'c' }, lastRun: { id: 'r', status: 'running' },
    kernelSnapshot: { schemaVersion: 1, runId: 'r', turnId: 'turn', state: 'running', lastEventSeq: '3', terminalWritten: false },
    messages: [], runtimeEvents: [start, event(2, 'run.phase', { phase: 'request_sent' })], toolCalls: [], approvals: [],
  } as unknown as ConversationDetail
  const notice = { schemaVersion: 1, runId: 'r', conversationId: 'c', turnId: 'turn', checkpointSeq: 2, revision: 1, text: '', reasoning: '分析' }
  const thought = applyKernelModelPreview(detail, notice)!
  expect(presentConversationActivity(thought, 'c').animation).toBe('thinking')
  const answer = applyKernelModelPreview(thought, { ...notice, revision: 2, text: '回复正文' })!
  expect(presentConversationActivity(answer, 'c').animation).toBe('writing')
  expect(presentConversationActivity(answer, 'other').label).toBe('准备就绪')
  const stale = { ...answer, kernelSnapshot: { ...answer.kernelSnapshot!, lastEventSeq: '5' } }
  expect(presentConversationActivity(stale, 'c').animation).toBe('receiving')
  const stopped = { ...answer, kernelSnapshot: { ...answer.kernelSnapshot!, state: 'cancelled' as const, terminalWritten: true } }
  expect(presentConversationActivity(stopped, 'c').label).toBe('已停止')
})
