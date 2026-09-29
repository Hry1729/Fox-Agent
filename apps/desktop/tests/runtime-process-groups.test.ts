import { expect, test } from 'bun:test'
import { activeProcessGroupTitle, processGroupTitle, projectRuntimeGroups, summarizeProcessActivity } from '../src/features/chat/runtime-process-groups'
import { answerDeltaFingerprint } from '../src/features/conversations/model/runtime-delta-fingerprint'
import type { RunEventRecord } from '../src/features/conversations/model/types'

const event = (seq: number, eventType: string, data: Record<string, unknown> = {}, runId = 'run'): RunEventRecord => ({
  runId, seq, eventType, event: { type: eventType, ...data }, createdAt: seq,
})
const answer = (seq: number, text: string) => event(seq, 'message.delta', {
  deltaLength: text.length, deltaFingerprint: answerDeltaFingerprint(text),
})

test('uses the same UTF-8 fingerprint as the Host for Unicode answer boundaries', () => {
  expect(answerDeltaFingerprint(' 🦊')).toBe('0e93180be4fd9c9e')
})

test('splits process and answer in original order, retaining stable keys through streaming', () => {
  const first = [event(1, 'reasoning.delta', { delta: 'plan' }), answer(2, '你好')]
  const before = projectRuntimeGroups('run', '你好', first)
  expect(before?.map(group => group.kind)).toEqual(['process', 'response'])
  const after = projectRuntimeGroups('run', '你好世界', [
    ...first, event(3, 'tool.started', { toolCallId: 'read-1', tool: 'read' }),
    event(4, 'tool.completed', { toolCallId: 'read-1', tool: 'read' }),
    event(5, 'reasoning.delta', { delta: 'done' }), answer(6, '世界'),
  ])!
  expect(after.map(group => group.kind)).toEqual(['process', 'response', 'process', 'response'])
  expect(after.filter(group => group.kind === 'response').map(group => group.text)).toEqual(['你好', '世界'])
  expect(after[0].key).toBe(before?.[0].key)
  expect(after[1].key).toBe(before?.[1].key)
  expect(after[2].kind === 'process' && after[2].events.map(item => item.seq)).toEqual([3, 4, 5])
})

test('keeps tool completion with its starting group across answer boundary', () => {
  const groups = projectRuntimeGroups('run', 'A', [
    event(1, 'tool.started', { toolCallId: 'x', tool: 'run_command' }),
    answer(2, 'A'),
    event(3, 'tool.completed', { toolCallId: 'x', tool: 'run_command' }),
  ])!
  expect(groups.map(group => group.kind)).toEqual(['process', 'response'])
  expect(groups[0].kind === 'process' && groups[0].events.map(item => item.seq)).toEqual([1, 3])
})

test('whitespace-only rounds do not scatter contiguous tools into separate headings', () => {
  const events = [
    event(1, 'reasoning.delta', { delta: 'plan' }), answer(2, '\n\n'),
    event(3, 'tool.started', { toolCallId: 'a', tool: 'read' }), answer(4, '\n\n'),
    event(5, 'tool.started', { toolCallId: 'b', tool: 'grep' }),
  ]
  const before = projectRuntimeGroups('run', '\n\n\n\n', events)!
  expect(before.map(group => group.kind)).toEqual(['process'])
  const after = projectRuntimeGroups('run', '\n\n\n\n现在开始生成报告。完成。\n', [
    ...events, answer(6, '现在开始生成报告。'),
    event(7, 'tool.completed', { toolCallId: 'a', tool: 'read' }),
    event(8, 'tool.started', { toolCallId: 'c', tool: 'write' }), answer(9, '完成。'),
    event(10, 'tool.started', { toolCallId: 'd', tool: 'read' }), answer(11, '\n'),
  ])!
  expect(after.map(group => group.kind)).toEqual(['process', 'response', 'process', 'response', 'process'])
  expect(after[0].key).toBe(before[0].key)
  expect(after[0].kind === 'process' && after[0].events.map(item => item.seq)).toEqual([1, 3, 5, 7])
  expect(after.filter(group => group.kind === 'response').map(group => group.text).join(''))
    .toBe('\n\n\n\n现在开始生成报告。完成。\n')
})

test('buffered whitespace preserves code indentation and never merges across a notice', () => {
  const groups = projectRuntimeGroups('run', '    code()\n', [
    event(1, 'tool.started', { toolCallId: 'a', tool: 'read' }), answer(2, '    '),
    event(3, 'run.retrying'), event(4, 'reasoning.delta', { delta: 'retry' }),
    answer(5, 'code()\n'),
  ])!
  expect(groups.map(group => group.kind)).toEqual(['process', 'notice', 'process', 'response'])
  expect(groups.at(-1)).toMatchObject({ text: '    code()\n' })
})

test('deduplicates replay, sorts late arrivals, and excludes another run', () => {
  const groups = projectRuntimeGroups('run', 'ok', [
    answer(3, 'ok'), event(1, 'reasoning.delta', { delta: 'think' }),
    answer(3, 'ok'), event(2, 'tool.started', { toolCallId: 'x', tool: 'read' }),
    event(1, 'reasoning.delta', { delta: 'unrelated' }, 'other'),
  ])!
  expect(groups.map(group => group.kind)).toEqual(['process', 'response'])
  expect(groups[0].kind === 'process' && groups[0].events).toHaveLength(2)
})

test('returns fallback for old or incomplete answer events without guessing order', () => {
  expect(projectRuntimeGroups('run', 'answer', [event(1, 'reasoning.delta', { delta: 'think' })])).toBeNull()
  expect(projectRuntimeGroups('run', 'answer', [answer(1, 'ans')])).toBeNull()
  expect(projectRuntimeGroups('run', 'answer', [answer(1, 'too long')])).toBeNull()
  expect(projectRuntimeGroups('run', 'answer', [event(1, 'message.delta', {})])).toBeNull()
  expect(projectRuntimeGroups('run', '🦊', [answer(1, '🦊')])?.[0]).toMatchObject({ text: '🦊' })
  expect(projectRuntimeGroups('run', 'xy', [answer(1, 'ab')])).toBeNull()
})

test('separates retry and question boundaries while preserving pure thinking', () => {
  const events = [event(1, 'reasoning.delta', { delta: 'first' }), event(2, 'run.retrying'),
    event(3, 'reasoning.delta', { delta: 'second' }), answer(4, 'A'),
    event(5, 'user.question.requested'), event(6, 'tool.started', { toolCallId: 'ask', tool: 'ask_user_question' })]
  const groups = projectRuntimeGroups('run', 'A', events)!
  expect(groups.map(group => group.kind)).toEqual(['process', 'notice', 'process', 'response', 'notice', 'process'])
  expect(groups[0].key).toBe('run:process:1')
  expect(groups[2].key).toBe('run:process:3')
  expect(groups[4]).toMatchObject({ label: '等待你的回答' })
})

test('ranks distinct call IDs by count and first appearance, including knowledge source', () => {
  const events = [
    event(1, 'tool.started', { toolCallId: 'a', tool: 'search_knowledge', input: { target: { source: 'remote', id: 'r' } } }),
    event(2, 'tool.completed', { toolCallId: 'a', tool: 'search_knowledge' }),
    event(3, 'tool.started', { toolCallId: 'b', tool: 'search_knowledge', input: { reference: { source: 'local', id: 'l' } } }),
    event(4, 'tool.started', { toolCallId: 'c', tool: 'query_kb' }),
    event(5, 'tool.started', { toolCallId: 'd', tool: 'read' }),
  ]
  expect(summarizeProcessActivity(events)).toEqual([
    { kind: 'remoteKnowledge', count: 2 }, { kind: 'localKnowledge', count: 1 }, { kind: 'read', count: 1 },
  ])
  expect(processGroupTitle(events)).toBe('检索了远程知识库，检索了本地知识库，读取了文件')
  expect(processGroupTitle([
    event(1, 'tool.started', { toolCallId: 'search', tool: 'search_code' }),
    event(2, 'tool.started', { toolCallId: 'web', tool: 'web_search' }),
  ])).toBe('已搜索代码并搜索网页')
  expect(processGroupTitle([])).toBe('已完成分析')
  expect(processGroupTitle([event(1, 'tool.started', { toolCallId: 'mixed', tool: 'search_knowledge', input: { targets: [{ source: 'local' }, { source: 'remote' }] } })])).toBe('检索了知识库')
  expect(processGroupTitle([event(1, 'tool.started', { toolCallId: 'mcp', tool: 'call_mcp_tool', input: { server: 'other' } })])).toBe('调用了工具')
  expect(processGroupTitle([event(1, 'tool.started', { toolCallId: 'office', tool: 'call_mcp_tool', input: { server: 'fox-office' } })])).toBe('调用了办公工具')
  expect(activeProcessGroupTitle()).toBe('正在分析请求')
  expect(activeProcessGroupTitle('search_code')).toBe('正在搜索代码')
})
