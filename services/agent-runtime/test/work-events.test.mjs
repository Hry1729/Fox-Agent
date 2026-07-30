import test from 'node:test'
import assert from 'node:assert/strict'
import {
  WORK_EVENT_SCHEMA_VERSION,
  WORK_EVENT_TYPES,
  createWorkEvent,
  createWorkEventDeduper,
  validateWorkEvent,
} from '../src/work-events.mjs'

test('defines the frozen A0 work event type set and common fields', () => {
  assert.deepEqual(WORK_EVENT_TYPES, [
    'goal.proposed', 'goal.activated', 'goal.blocked', 'goal.completed', 'goal.cancelled',
    'task.created', 'task.started', 'task.completed', 'task.blocked', 'task.interrupted',
    'evidence.added', 'evidence.validated',
  ])
  const event = createWorkEvent({
    type: 'task.started',
    conversationId: 'conversation-1',
    goalId: 'goal-1',
    taskId: 'task-1',
    runId: 'run-1',
    traceId: 'trace-1',
    spanId: 'span-1',
    sequence: 7,
  })
  assert.equal(event.schemaVersion, WORK_EVENT_SCHEMA_VERSION)
  assert.equal(validateWorkEvent(event), null)
  assert.deepEqual(Object.keys(event), [
    'type', 'schemaVersion', 'conversationId', 'goalId', 'taskId', 'runId',
    'traceId', 'spanId', 'sequence', 'timestamp', 'data',
  ])
})

test('deduplicates by conversation and sequence and safely ignores unknown types', () => {
  const deduper = createWorkEventDeduper()
  const event = createWorkEvent({
    type: 'goal.proposed',
    conversationId: 'conversation-1',
    goalId: 'goal-1',
    sequence: 1,
  })
  assert.equal(deduper.accept(event), true)
  assert.equal(deduper.accept({ ...event, type: 'goal.activated' }), false)
  assert.equal(deduper.accept({ ...event, conversationId: 'conversation-2' }), true)
  assert.equal(deduper.accept({ ...event, type: 'future.work.event' }), false)
})
