// Kernel work tool arguments (fix round, 2026-10-20; was the diagnosis-round
// reproduction). A provider may serialize `plan_revision_create.tasks` as a
// JSON string. The legacy Node tool repaired that in `prepareArguments`; the
// Kernel paths now apply the very same preparer to the round output BEFORE it
// is committed to the Host, so the durable proposal (canonical input, approval
// identity, dispatch payload) is the prepared array. Anything that does not
// parse to an array is left unchanged and the Host refuses it as data.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createHostTools } from '../src/host-tools.mjs'
import { installKernelHostTools, prepareRoundOutput } from '../src/pi-kernel-loop.mjs'
import {
  KERNEL_TOOL_ARGUMENT_PREPARERS,
  prepareKernelToolCallArguments,
  preparePlanRevisionArguments,
} from '../src/tool-argument-preparers.mjs'

const tasks = [{ title: '计算 17*23+5', ordinal: 0 }]
const serializedTasks = JSON.stringify(tasks)

function planCall(id, args) {
  return { type: 'toolCall', id, name: 'plan_revision_create', arguments: args }
}

test('legacy and Kernel share one plan_revision_create preparer', () => {
  const legacy = createHostTools(() => {}).find(({ name }) => name === 'plan_revision_create')
  assert.equal(legacy.prepareArguments, preparePlanRevisionArguments)
  assert.equal(KERNEL_TOOL_ARGUMENT_PREPARERS.plan_revision_create, preparePlanRevisionArguments)
  assert.deepEqual(Object.keys(KERNEL_TOOL_ARGUMENT_PREPARERS), ['plan_revision_create'],
    'scope follows legacy preparePlanRevisionArguments only')
  const prepared = legacy.prepareArguments({ goalId: 'g', title: 't', summary: 's', tasks: serializedTasks })
  assert.deepEqual(prepared.tasks, tasks)
})

test('preparer is strict: only a string that parses to an array is replaced', () => {
  for (const value of ['[{', '{"title":"x"}', '"[]"', 'not json', `  ${'['.repeat(1)}1,2`, '[' + ' '.repeat(400_001) + ']']) {
    const args = { goalId: 'g', title: 't', summary: 's', tasks: value }
    assert.equal(preparePlanRevisionArguments(args), args, `kept unchanged: ${value.slice(0, 20)}`)
  }
  const arrayArgs = { goalId: 'g', tasks }
  assert.equal(preparePlanRevisionArguments(arrayArgs), arrayArgs, 'an array is never touched')
  assert.deepEqual(preparePlanRevisionArguments({ tasks: `  ${serializedTasks}\n` }).tasks, tasks)
  assert.equal(preparePlanRevisionArguments(null), null)
  assert.equal(preparePlanRevisionArguments('x'), 'x')
})

test('Kernel live-loop tool carries the same preparer; other tools carry none', () => {
  const session = { isIdle: true, agent: { state: { tools: [] }, subscribe() {}, abort() {} } }
  installKernelHostTools(session, [
    { name: 'plan_revision_create', description: 'plan revision under test',
      parameters: { type: 'object', properties: { tasks: { type: 'array' } }, required: ['tasks'] } },
    { name: 'task_evidence_add', description: 'evidence under test', parameters: { type: 'object' } },
  ], { settledFor: () => ({ content: [], isError: false }) })
  const byName = new Map(session.agent.state.tools.map(tool => [tool.name, tool]))
  assert.equal(byName.get('plan_revision_create').prepareArguments, preparePlanRevisionArguments)
  assert.equal(byName.get('task_evidence_add').prepareArguments, undefined)
  assert.deepEqual(byName.get('plan_revision_create').parameters, { type: 'object' },
    'engine-side validation stays permissive: the Host remains the authority')
})

test('round output committed to the Host carries the prepared array (and the transcript agrees)', () => {
  const message = {
    role: 'assistant', stopReason: 'toolUse',
    content: [
      planCall('call-plan', { goalId: 'g', title: 't', summary: 's', tasks: serializedTasks }),
      planCall('call-bad', { goalId: 'g', title: 't', summary: 's', tasks: '[{' }),
      { type: 'toolCall', id: 'call-other', name: 'task_evidence_add', arguments: { refId: '["kept"]' } },
    ],
  }
  assert.equal(prepareKernelToolCallArguments(message), 1)
  const frame = prepareRoundOutput(message, { turnId: 'turn-1' })
  const calls = frame.assistantMessage.content
  assert.deepEqual(calls[0].arguments.tasks, tasks, 'durable proposal input is the array')
  assert.equal(calls[1].arguments.tasks, '[{', 'malformed string stays for the Host to refuse as data')
  assert.equal(calls[2].arguments.refId, '["kept"]', 'tools outside the legacy scope are untouched')
  assert.deepEqual(message.content[0].arguments.tasks, tasks, 'worker transcript equals the Host proposal')
  // Idempotent: a second pass (e.g. the engine-side prepareArguments) changes nothing.
  assert.equal(prepareKernelToolCallArguments(message), 0)
})

test('single-round transport commits the same prepared array', async () => {
  const { prepareKernelModelResponse } = await import('../src/pi-kernel-batch-resume.mjs')
  const prepared = { initial: true, runId: 'run', turnId: 'turn', checkpointSeq: 1, requireCompletion: false }
  const message = { role: 'assistant', stopReason: 'toolUse',
    content: [planCall('call-plan', { goalId: 'g', title: 't', summary: 's', tasks: serializedTasks })] }
  const response = prepareKernelModelResponse(message, prepared)
  assert.deepEqual(response.assistantMessage.content[0].arguments.tasks, tasks)
})
