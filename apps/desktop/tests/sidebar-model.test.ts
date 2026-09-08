import { describe, expect, test } from 'bun:test'
import {
  childRunCounts,
  childRunIsActive,
  childRunStatusLabel,
  childRunStatusPresentation,
  formatDurationMs,
  formatRelativeUpdate,
  latestConversationContextUsage,
  normalizeBrowserUrl,
  webActivitySummary,
} from '../src/features/chat/sidebar-model'
import type { ChildRunRecord, RunEventRecord, ToolCallRecord } from '../src/features/conversations/model/types'

const run = (status: ChildRunRecord['status']): ChildRunRecord => ({
  id: `delegation-${status}`,
  parentRunId: 'parent',
  childRunId: `child-${status}`,
  childConversationId: `conversation-${status}`,
  workerAgentId: 'worker',
  workerAgentName: 'Worker',
  objective: 'Inspect the project',
  context: '',
  teamRunId: null,
  teamMemberId: null,
  allowedTools: null,
  status,
  depth: 1,
  budget: { maxDurationMs: 60_000, maxTotalTokens: 4_000, maxOutputTokens: 2_000, maxToolCalls: 8 },
  resultText: null,
  inputTokens: 0,
  outputTokens: 0,
  totalTokens: 0,
  toolCallCount: 0,
  errorCode: null,
  errorMessage: null,
  createdAt: 1_000,
  startedAt: null,
  finishedAt: null,
})

const usageEvent = (runId: string, seq: number, event: Record<string, number>): RunEventRecord => ({
  runId,
  seq,
  eventType: 'usage.updated',
  event: { type: 'usage.updated', ...event },
  createdAt: seq,
} as RunEventRecord)

describe('right sidebar model', () => {
  test('treats queued, running, and cancelling Child Runs as active', () => {
    expect(childRunIsActive(run('queued'))).toBe(true)
    expect(childRunIsActive(run('running'))).toBe(true)
    expect(childRunIsActive(run('cancelling'))).toBe(true)
    expect(childRunIsActive(run('completed'))).toBe(false)
    expect(childRunCounts([run('running'), run('completed'), run('failed')])).toEqual({ active: 1, completed: 1, failed: 1 })
  })

  test('formats short and long elapsed times for people', () => {
    expect(formatDurationMs(400)).toBe('< 1 秒')
    expect(formatDurationMs(65_000)).toBe('1 分 5 秒')
    expect(formatDurationMs(7_200_000)).toBe('2 小时')
  })

  test('presents only persisted Child Run states in the realtime status bar', () => {
    expect(childRunStatusLabel('queued')).toBe('已接收')
    expect(childRunStatusLabel('running')).toBe('运行中')
    expect(childRunStatusLabel('failed')).toBe('异常结束')
    expect(childRunStatusPresentation('running')).toEqual({
      label: '运行中',
      detail: '子 Agent 正在执行任务，状态会自动更新',
      tone: 'info',
    })
    expect(childRunStatusPresentation('failed').tone).toBe('danger')
  })

  test('formats the last persisted status update without inventing workflow steps', () => {
    const now = 100_000_000
    expect(formatRelativeUpdate(now - 20_000, now)).toBe('刚刚更新')
    expect(formatRelativeUpdate(now - 5 * 60_000, now)).toBe('5 分钟前更新')
    expect(formatRelativeUpdate(now - 2 * 60 * 60_000, now)).toBe('2 小时前更新')
  })

  test('normalizes only HTTP and HTTPS browser addresses', () => {
    expect(normalizeBrowserUrl('example.com')).toBe('https://example.com/')
    expect(normalizeBrowserUrl('http://localhost:3000/docs')).toBe('http://localhost:3000/docs')
    expect(normalizeBrowserUrl('file:///C:/secret.txt')).toBeNull()
    expect(normalizeBrowserUrl('javascript:alert(1)')).toBeNull()
  })

  test('turns persisted Agent web calls into inspectable browser activity', () => {
    const tool = { toolName: 'web_search', input: { query: 'Fox agent' } } as ToolCallRecord
    expect(webActivitySummary(tool)).toEqual({
      label: '网页搜索',
      target: 'Fox agent',
      navigableUrl: 'https://duckduckgo.com/?q=Fox%20agent',
    })
  })

  test('shows the latest model request as context usage instead of cumulative run billing', () => {
    const usage = latestConversationContextUsage([
      usageEvent('run-1', 2, { inputTokens: 160_000, outputTokens: 20_000, cacheReadTokens: 20_000, cacheWriteTokens: 0, totalTokens: 200_000 }),
      usageEvent('run-1', 5, { inputTokens: 260_000, outputTokens: 50_000, cacheReadTokens: 42_000, cacheWriteTokens: 0, totalTokens: 352_000 }),
    ])
    expect(usage).toEqual({ inputTokens: 100_000, outputTokens: 30_000, cacheReadTokens: 22_000, cacheWriteTokens: 0, totalTokens: 152_000 })
  })

  test('does not subtract usage across different runs or legacy counter resets', () => {
    expect(latestConversationContextUsage([
      usageEvent('run-1', 2, { inputTokens: 90, outputTokens: 10, cacheReadTokens: 0, cacheWriteTokens: 0, totalTokens: 100 }),
      usageEvent('run-2', 2, { inputTokens: 45, outputTokens: 5, cacheReadTokens: 0, cacheWriteTokens: 0, totalTokens: 50 }),
    ]).totalTokens).toBe(50)
    expect(latestConversationContextUsage([
      usageEvent('legacy', 2, { inputTokens: 90, outputTokens: 10, cacheReadTokens: 0, cacheWriteTokens: 0, totalTokens: 100 }),
      usageEvent('legacy', 3, { inputTokens: 36, outputTokens: 4, cacheReadTokens: 0, cacheWriteTokens: 0, totalTokens: 40 }),
    ]).totalTokens).toBe(40)
  })
})
