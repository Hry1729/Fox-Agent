import { describe, expect, test } from 'bun:test'
import { runtimeProcess, toolPartialOutputText, withSettledToolCalls, TOOL_HOST_SETTLED_EVENT, type ToolCallLike } from '../src/features/chat/timeline/fox-runtime-process-model'
import { summarizeRunSteps } from '../src/features/chat/run-activity-presentation'
import type { RunEventRecord } from '../src/features/conversations/model/types'

// 13.38（A-2）：取消 / 中断后，工具卡定格，取消前已产生的输出可见。
const event = (seq: number, eventType: string, data: Record<string, unknown> = {}, runId = 'run'): RunEventRecord => ({
  runId, seq, eventType, event: { type: eventType, ...data }, createdAt: seq,
})

const ping = [
  event(1, 'run.started'),
  event(2, 'tool.started', { toolCallId: 'p1', tool: 'bash', input: { command: 'ping -n 60 127.0.0.1' } }),
  event(3, 'tool.updated', { toolCallId: 'p1', update: { status: 'in_progress', text: '来自 127.0.0.1 的回复\n', streamedOutput: true } }),
]

describe('runtimeProcess settles unfinished tools when the run ends first', () => {
  test('a cancelled run freezes the running tool as 已取消 and keeps its partial output', () => {
    const process = runtimeProcess([...ping, event(4, 'run.cancelled')])
    const tool = process.tools.find(item => item.id === 'p1' || (item as { toolCallId?: string }).toolCallId === 'p1') ?? process.tools[0]
    expect(tool.completed).toBe(false)
    expect(tool.settled).toBe('cancelled')
    expect(toolPartialOutputText(tool.partialOutput)).toBe('来自 127.0.0.1 的回复\n')
  })

  test('a failed or interrupted run freezes it as 已中断; a running run does not freeze it', () => {
    expect(runtimeProcess([...ping, event(4, 'run.failed', { code: 'x', message: 'y' })]).tools[0].settled).toBe('interrupted')
    expect(runtimeProcess([...ping, event(4, 'run.interrupted')]).tools[0].settled).toBe('interrupted')
    const live = runtimeProcess(ping).tools[0]
    expect(live.settled).toBeUndefined()
    expect(toolPartialOutputText(live.partialOutput)).toContain('127.0.0.1')
  })

  test('an update without readable output does not erase earlier output', () => {
    const tool = runtimeProcess([...ping, event(4, 'tool.updated', { toolCallId: 'p1', update: { status: 'in_progress' } })]).tools[0]
    expect(toolPartialOutputText(tool.partialOutput)).toBe('来自 127.0.0.1 的回复\n')
  })
})

describe('withSettledToolCalls', () => {
  const hostCall: ToolCallLike = {
    runId: 'run', runtimeToolCallId: 'h1', toolName: 'run_command', input: { command: 'ping' },
    status: 'cancelled', result: { status: 'running', text: '正在 Ping 127.0.0.1\n', streamedOutput: true }, errorMessage: null,
  }

  test('adds the database card of a cancelled Host tool with its partial output', () => {
    const events = [event(1, 'run.started'), event(2, 'tool.started', { toolCallId: 'h1', tool: 'run_command', input: { command: 'ping' } }), event(3, 'run.cancelled')]
    const merged = withSettledToolCalls(events, [hostCall])
    expect(merged).toHaveLength(4)
    expect(merged[3].eventType).toBe(TOOL_HOST_SETTLED_EVENT)
    expect(merged[3].seq).toBe(3)
    const tool = runtimeProcess(merged).tools[0]
    expect(tool.settled).toBe('cancelled')
    expect(toolPartialOutputText(tool.partialOutput)).toBe('正在 Ping 127.0.0.1\n')
  })

  test('skips cards of other runs, finished cards, and cards the stream already ended', () => {
    const events = [event(1, 'run.started'), event(2, 'tool.completed', { toolCallId: 'h1', result: {} })]
    expect(withSettledToolCalls(events, [hostCall])).toBe(events)
    expect(withSettledToolCalls([event(1, 'run.started')], [{ ...hostCall, runId: 'other' }])).toHaveLength(1)
    expect(withSettledToolCalls([event(1, 'run.started')], [{ ...hostCall, status: 'completed' }])).toHaveLength(1)
    expect(withSettledToolCalls([], [hostCall])).toEqual([])
  })

  test('falls back to the error message when the card has no output', () => {
    const events = [event(1, 'run.started'), event(2, 'tool.started', { toolCallId: 'h1', tool: 'run_command' }), event(3, 'run.failed')]
    const merged = withSettledToolCalls(events, [{ ...hostCall, status: 'interrupted', result: null, errorMessage: '运行中断' }])
    const tool = runtimeProcess(merged).tools[0]
    expect(tool.settled).toBe('interrupted')
    expect(toolPartialOutputText(tool.partialOutput)).toBe('运行中断')
  })
})

test('the run summary counts unfinished tools of a cancelled run as 已取消', () => {
  expect(summarizeRunSteps([...ping, event(4, 'run.cancelled')], false)).toContain('1 项已取消')
  expect(summarizeRunSteps([...ping, event(4, 'run.failed')], false)).toContain('1 项未完成')
})

test('toolPartialOutputText reads text, content parts, output and plain strings', () => {
  expect(toolPartialOutputText('a')).toBe('a')
  expect(toolPartialOutputText({ content: [{ text: 'a' }, { text: 'b' }] })).toBe('a\nb')
  expect(toolPartialOutputText({ output: 'o' })).toBe('o')
  expect(toolPartialOutputText(null)).toBe('')
  expect(toolPartialOutputText([1])).toBe('')
})
