import { describe, expect, test } from 'bun:test'
import {
  LONG_TASK_MS,
  PROVISIONAL_FRAME_BUDGET_MS,
  ProfilingCollector,
  RingBuffer,
  percentile,
  summarize,
} from '../src/features/profiling/telemetry'
import { buildHistoryDetail, buildStreamScenario } from '../src/features/profiling/fixtures'

describe('percentile / summarize', () => {
  test('percentile returns order-statistic values', () => {
    const values = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100]
    expect(percentile(values, 50)).toBe(50)
    expect(percentile(values, 95)).toBe(100)
    expect(percentile([], 95)).toBe(0)
  })

  test('summarize computes p50/p95/max/mean', () => {
    const result = summarize([30, 10, 20])
    expect(result.count).toBe(3)
    expect(result.p50).toBe(20)
    expect(result.max).toBe(30)
    expect(result.mean).toBeCloseTo(20, 5)
  })

  test('threshold constants', () => {
    expect(LONG_TASK_MS).toBe(50)
    expect(PROVISIONAL_FRAME_BUDGET_MS).toBe(16.7)
  })
})

describe('RingBuffer (O(1) circular)', () => {
  test('preserves FIFO order and evicts oldest past capacity', () => {
    const buffer = new RingBuffer<number>(3)
    for (const value of [1, 2, 3, 4, 5]) buffer.push(value)
    expect(buffer.toArray()).toEqual([3, 4, 5])
    expect(buffer.size).toBe(3)
  })

  test('clear empties and keeps capacity', () => {
    const buffer = new RingBuffer<number>(2)
    buffer.push(1)
    buffer.push(2)
    buffer.clear()
    expect(buffer.size).toBe(0)
    buffer.push(9)
    expect(buffer.toArray()).toEqual([9])
  })
})

describe('scenario instance isolation', () => {
  test('two consecutive same-kind scenarios get distinct runIds and do not share samples', () => {
    const c = new ProfilingCollector()
    c.startSampling()
    c.beginScenario('history-mount', 'markdown', { messageCount: 100 })
    c.onRender('conversation-timeline', 'mount', 10, 100, 0, 10)
    const a = c.endScenario()

    c.beginScenario('history-mount', 'markdown', { messageCount: 200 })
    c.onRender('conversation-timeline', 'mount', 12, 100, 20, 32)
    c.onRender('conversation-timeline', 'update', 4, 60, 40, 44)
    const b = c.endScenario()

    expect(a.scenarioRunId).not.toBe(b.scenarioRunId)
    expect(a.renderCommitCount).toBe(1)
    expect(b.renderCommitCount).toBe(2)
    expect(a.messageCount).toBe(100)
    expect(b.messageCount).toBe(200)
  })

  test('different kind/size do not accumulate', () => {
    const c = new ProfilingCollector()
    c.startSampling()
    c.beginScenario('history-mount', 'markdown', { messageCount: 100 })
    c.onRender('conversation-timeline', 'mount', 10, 100, 0, 10)
    c.endScenario()
    c.beginScenario('history-mount', 'tools', { size: 500, messageCount: 500 })
    c.onRender('conversation-timeline', 'mount', 20, 100, 0, 20)
    const tools = c.endScenario()
    expect(tools.kind).toBe('tools')
    expect(tools.renderCommitCount).toBe(1)
    expect(tools.size).toBe(500)
    expect(tools.messageCount).toBe(500)
  })

  test('onRender is ignored while idle / between scenarios (no buffer pollution)', () => {
    const c = new ProfilingCollector()
    c.startSampling()
    // No active scenario yet — this idle commit must be dropped.
    c.onRender('conversation-timeline', 'update', 99, 100, 0, 99)
    c.beginScenario('history-mount', 'markdown', { messageCount: 10 })
    c.onRender('conversation-timeline', 'mount', 5, 10, 0, 5)
    c.endScenario()
    // After endScenario, activeRunId is null — this commit is dropped.
    c.onRender('conversation-timeline', 'update', 88, 100, 0, 88)
    c.beginScenario('stream-replay', 'markdown', { eventCount: 3 })
    c.onRender('conversation-timeline', 'update', 3, 10, 0, 3)
    const b = c.endScenario()
    expect(b.renderCommitCount).toBe(1)
  })
})

describe('Long Task time-window attribution', () => {
  test('delivered tasks attribute by startTime, not callback-time scenario', () => {
    const c = new ProfilingCollector()
    c.startSampling()
    c.beginScenario('history-mount', 'markdown', { messageCount: 100 })
    const inA = performance.now()
    c.ingestLongTask(inA, 60)
    const a = c.endScenario()
    expect(a.longTasks.count).toBe(1)

    c.beginScenario('stream-replay', 'tools', { eventCount: 5 })
    const inB = performance.now()
    // Late delivery for A (startTime inside A's closed window) must NOT land on B.
    c.ingestLongTask(inA, 70)
    c.ingestLongTask(inB, 55)
    const b = c.endScenario()
    expect(b.longTasks.count).toBe(1)
  })

  test('tasks outside any scenario window are dropped', () => {
    const c = new ProfilingCollector()
    // No active scenario / window — nothing to attribute to.
    c.ingestLongTask(performance.now(), 999)
    c.beginScenario('history-mount', 'markdown', {})
    const report = c.endScenario()
    expect(report.longTasks.count).toBe(0)
  })
})

describe('report metadata and redaction', () => {
  test('report carries schema/build/scenario metadata and no message content', () => {
    const c = new ProfilingCollector()
    c.startSampling()
    c.beginScenario('history-mount', 'code-cold', { size: 500, coldWarm: 'cold', messageCount: 500 })
    c.onRender('conversation-timeline', 'mount', 8, 100, 0, 8)
    c.endScenario()
    const json = c.exportJson()
    expect(json).toContain('"schemaVersion": 2')
    expect(json).toContain('"scenarioRunId"')
    expect(json).toContain('"devicePixelRatio"')
    expect(json).toContain('"coldWarm": "cold"')
    expect(json).not.toContain('请处理第')
    expect(json).not.toContain('fixture-conversation')
    expect(json).not.toContain('src/mod-')
  })
})

describe('fixture semantics', () => {
  test('messageCount is the total message count for 100/500/1000', () => {
    for (const total of [100, 500, 1000]) {
      const detail = buildHistoryDetail('mixed', total)
      expect(detail.messages.length).toBe(total)
      expect(detail.messages.filter((m) => m.role === 'user').length).toBe(Math.floor(total / 2))
    }
  })

  test('tools history emits tool lifecycle runtime events', () => {
    const detail = buildHistoryDetail('tools', 100)
    const types = new Set(detail.runtimeEvents.map((event) => event.eventType))
    expect(types.has('tool.started')).toBe(true)
    expect(types.has('tool.completed')).toBe(true)
  })

  test('markdown history has no tool events; code answers contain fenced blocks', () => {
    const markdown = buildHistoryDetail('markdown', 100)
    expect(markdown.runtimeEvents.length).toBe(0)
    expect(markdown.messages.some((m) => m.content.includes('```'))).toBe(false)

    const code = buildHistoryDetail('code', 100)
    expect(code.messages.some((m) => m.content.includes('```typescript'))).toBe(true)
  })

  test('code stream final text contains a fenced code block', () => {
    const scenario = buildStreamScenario('code', 100)
    expect(scenario.finalText).toContain('```typescript')
    expect(scenario.finalText).toContain('```')
  })

  test('tools stream includes full run/message/tool lifecycle events', () => {
    const scenario = buildStreamScenario('tools')
    const types = scenario.notifications.map((n) => n.event.type)
    for (const required of ['run.started', 'message.started', 'tool.started', 'tool.completed', 'message.delta', 'message.completed', 'run.completed']) {
      expect(types).toContain(required)
    }
  })

  test('stream event counts are positive and deltas reconstruct the final text', () => {
    const scenario = buildStreamScenario('markdown', 120)
    expect(scenario.eventCount).toBe(scenario.notifications.length)
    const deltas = scenario.notifications
      .filter((n) => n.event.type === 'message.delta')
      .map((n) => String(n.event.delta ?? ''))
      .join('')
    expect(deltas).toBe(scenario.finalText)
  })
})
