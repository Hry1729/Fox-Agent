import test from 'node:test'
import assert from 'node:assert/strict'
import { createFoxPlanningExtension, planningContextMessage } from '../src/fox-planning-extension.mjs'

test('injects Host-owned project and persisted work context', () => {
  const message = planningContextMessage({
    projectRoot: 'D:\\projects\\demo',
    permissionMode: 'ask_before_write',
    workSnapshot: {
      goal: { id: 'goal-uuid', status: 'active', version: 2 },
      tasks: [{ id: 'task-uuid', status: 'queued', version: 1 }],
      evidence: [],
    },
  })

  assert.ok(message.includes('D:\\projects\\demo'))
  assert.match(message, /conversation identity is Host-owned/)
  assert.match(message, /"id": "goal-uuid"/)
  assert.match(message, /"id": "task-uuid"/)
})

test('registers Fox work tools through the Coding Agent extension lifecycle', async () => {
  const registered = []
  const handlers = new Map()
  const extension = createFoxPlanningExtension({
    workTools: [{ name: 'goal_propose' }, { name: 'task_create_many' }],
    context: { projectRoot: 'D:\\projects\\demo' },
  })

  await extension({
    registerTool: (tool) => registered.push(tool.name),
    on: (event, handler) => handlers.set(event, handler),
  })

  assert.deepEqual(registered, ['goal_propose', 'task_create_many'])
  const injected = await handlers.get('before_agent_start')()
  assert.equal(injected.message.customType, 'fox-host-planning-context')
  assert.equal(injected.message.display, false)
})
