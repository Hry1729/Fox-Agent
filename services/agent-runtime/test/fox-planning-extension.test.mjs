import test from 'node:test'
import assert from 'node:assert/strict'
import {
  compactWorkSnapshot,
  createFoxPlanningExtension,
} from '../src/fox-planning-extension.mjs'

test('preserves WorkSnapshot V2 planning state and root source identity', () => {
  const compact = compactWorkSnapshot({
    sourceHighWatermark: { eventSequence: 42 },
    sourceHash: 'sha256:work-snapshot',
    details: {
      schemaVersion: 2,
      goal: { id: 'goal-v2' },
      tasks: [{ id: 'task-v2' }],
      evidence: [{ id: 'evidence-v2' }],
      planRevisions: [
        { id: 'plan-1', revision: 1 },
        { id: 'plan-3', revision: 3 },
        { id: 'plan-2', revision: 2 },
      ],
      reviewFindings: [
        { id: 'resolved-newer', status: 'resolved', createdAt: 40 },
        { id: 'open-older', status: 'open', createdAt: 10 },
        { id: 'open-newer', status: 'OPEN', createdAt: 30 },
      ],
      acceptances: [
        { id: 'acceptance-older', createdAt: 10 },
        { id: 'acceptance-newer', createdAt: 20 },
      ],
      taskLedger: { schemaVersion: 1, activeTasks: [{ taskId: 'task-v2' }] },
    },
  })

  assert.deepEqual(compact.sourceHighWatermark, { eventSequence: 42 })
  assert.equal(compact.sourceHash, 'sha256:work-snapshot')
  assert.deepEqual(compact.planRevisions.map((item) => item.id), ['plan-3', 'plan-2', 'plan-1'])
  assert.deepEqual(compact.reviewFindings.map((item) => item.id), ['open-newer', 'open-older'])
  assert.deepEqual(compact.acceptances.map((item) => item.id), ['acceptance-newer', 'acceptance-older'])
  assert.equal(compact.taskLedger.activeTasks[0].taskId, 'task-v2')
})

test('bounds WorkSnapshot V2 history to the latest planning records', () => {
  const compact = compactWorkSnapshot({
    schemaVersion: 2,
    planRevisions: Array.from({ length: 12 }, (_, index) => ({ id: `plan-${index + 1}`, revision: index + 1 })),
    reviewFindings: Array.from({ length: 20 }, (_, index) => ({ id: `finding-${index + 1}`, status: 'open', createdAt: index + 1 })),
    acceptances: Array.from({ length: 12 }, (_, index) => ({ id: `acceptance-${index + 1}`, createdAt: index + 1 })),
  })

  assert.equal(compact.planRevisions.length, 8)
  assert.deepEqual(compact.planRevisions.map((item) => item.revision), [12, 11, 10, 9, 8, 7, 6, 5])
  assert.equal(compact.reviewFindings.length, 16)
  assert.deepEqual(compact.reviewFindings.map((item) => item.createdAt), [20, 19, 18, 17, 16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5])
  assert.equal(compact.acceptances.length, 8)
  assert.deepEqual(compact.acceptances.map((item) => item.createdAt), [12, 11, 10, 9, 8, 7, 6, 5])
})

test('keeps the WorkSnapshot V1 compact projection backward compatible', () => {
  assert.deepEqual(compactWorkSnapshot({
    schemaVersion: 1,
    goal: { id: 'goal-v1' },
    tasks: [{ id: 'task-v1' }],
    evidence: [{ id: 'evidence-v1' }],
    planRevisions: [{ id: 'ignored-v2-plan' }],
    reviewFindings: [{ id: 'ignored-v2-finding', status: 'open' }],
    acceptances: [{ id: 'ignored-v2-acceptance' }],
    taskLedger: null,
  }), {
    schemaVersion: 1,
    goal: { id: 'goal-v1' },
    tasks: [{ id: 'task-v1' }],
    evidence: [{ id: 'evidence-v1' }],
    taskLedger: null,
  })
})

test('registers Fox work tools through the Coding Agent extension lifecycle', async () => {
  const registered = []
  const handlers = new Map()
  const workSnapshot = { schemaVersion: 2, goal: { id: 'goal-runtime' } }
  const extension = createFoxPlanningExtension({
    workTools: [{ name: 'goal_propose' }, { name: 'task_create_many' }],
    context: { projectRoot: 'D:\\projects\\demo', workSnapshot },
  })

  await extension({
    registerTool: (tool) => registered.push(tool.name),
    on: (event, handler) => handlers.set(event, handler),
  })

  assert.deepEqual(registered, ['goal_propose', 'task_create_many'])
  assert.equal(handlers.has('before_agent_start'), false)
})
