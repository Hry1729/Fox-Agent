import { describe, expect, test } from 'bun:test'
import {
  childRunCounts,
  childRunIsActive,
  formatDurationMs,
  normalizeBrowserUrl,
  webActivitySummary,
} from '../src/features/chat/sidebar-model'
import type { ChildRunRecord, ToolCallRecord } from '../src/features/conversations/model/types'

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
})
