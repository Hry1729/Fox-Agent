import { expect, test } from 'bun:test'
import { runtimeProcessActivity } from '../src/features/chat/runtime-process-activity'
import type { RunEventRecord } from '../src/features/conversations/model/types'

const event = (seq: number, eventType: string, value: Record<string, unknown> = {}): RunEventRecord => ({
  runId: 'run', seq, eventType, event: { type: eventType, ...value }, createdAt: seq,
})

test('only the current phase glows across reasoning, tools and model continuation', () => {
  const events = [event(1, 'reasoning.delta', { delta: 'Analyze the data' })]
  const activity = () => runtimeProcessActivity(events, true)
  expect(activity().reasoning).toBe(true)
  events.push(event(2, 'tool.started', { toolCallId: 'compute' }))
  expect(activity().reasoning).toBe(false)
  expect([...activity().activeToolIds]).toEqual(['compute'])
  events.push(event(3, 'tool.completed', { toolCallId: 'compute' }))
  expect(activity().reasoning).toBe(false)
  expect(activity().activeToolIds.size).toBe(0)
  events.push(event(4, 'reasoning.delta', { delta: 'Check the result' }))
  expect(activity().reasoning).toBe(true)
  events.push(event(5, 'message.delta', { delta: 'The result is' }))
  expect(activity().reasoning).toBe(false)
  events.push(event(6, 'reasoning.delta', { delta: 'new thought' }))
  events.push(event(7, 'message.delta', { deltaLength: 2 }))
  expect(activity().reasoning).toBe(false)
})

test('parallel tools stay active independently and suppress reasoning', () => {
  const events = [event(1, 'tool.started', { toolCallId: 'a' }), event(2, 'tool.started', { toolCallId: 'b' }),
    event(3, 'tool.completed', { toolCallId: 'a' }), event(4, 'reasoning.delta', { delta: 'Waiting' })]
  const result = runtimeProcessActivity(events, true)
  expect([...result.activeToolIds]).toEqual(['b'])
  expect(result.reasoning).toBe(false)
})

for (const terminal of ['run.completed', 'run.failed', 'run.cancelled', 'run.interrupted']) {
  test(`${terminal} stops all animation even with a stale running flag`, () => {
    const events = [event(1, 'tool.started', { toolCallId: 'unfinished' }), event(2, terminal),
      event(3, 'reasoning.delta', { delta: 'historical thought', source: 'yuxi-history' })]
    const result = runtimeProcessActivity(events, true)
    expect(result.running).toBe(false)
    expect(result.reasoning).toBe(false)
    expect(result.activeToolIds.size).toBe(0)
  })
}

test('paused runs and synthetic history after an answer never glow', () => {
  const events = [event(1, 'reasoning.delta', { delta: 'history', source: 'yuxi-history' })]
  expect(runtimeProcessActivity(events, false).reasoning).toBe(false)
  expect(runtimeProcessActivity(events, true, true).reasoning).toBe(false)
  events.push(event(2, 'reasoning.delta', { delta: 'real continuation' }))
  expect(runtimeProcessActivity(events, true, true).reasoning).toBe(true)
})

test('source and empty streaming events do not restart or end a thought', () => {
  const events = [event(1, 'reasoning.delta', { delta: 'reasoning' }), event(2, 'source.added'),
    event(3, 'message.delta', { delta: '' }), event(4, 'reasoning.delta', { delta: ' ' })]
  expect(runtimeProcessActivity(events, true).reasoning).toBe(true)
  events.push(event(5, 'message.completed'))
  expect(runtimeProcessActivity(events, true).reasoning).toBe(false)
})
