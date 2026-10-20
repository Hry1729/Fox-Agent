// Reproduction (diagnosis round, 2026-10-20): on the legacy path the Node
// host tool `plan_revision_create` repairs a model that serializes `tasks` as a
// JSON string (`prepareArguments`). The Kernel live loop installs its own
// permissive replay tools from the Host's definitions, without that preparer,
// so the provider's raw arguments reach the Host unchanged and the Host's
// `validate_plan_tasks` rejects them as `plan.tasks_required`.
//
// This pins the asymmetry only; it changes no behavior.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createHostTools } from '../src/host-tools.mjs'
import { installKernelHostTools } from '../src/pi-kernel-loop.mjs'

const serializedTasks = '[{"title":"计算 17*23+5","ordinal":0}]'

test('legacy host tool repairs a JSON-string task list before execution', () => {
  const legacy = createHostTools(() => {}).find(({ name }) => name === 'plan_revision_create')
  assert.equal(typeof legacy.prepareArguments, 'function')
  const prepared = legacy.prepareArguments({ goalId: 'g', title: 't', summary: 's', tasks: serializedTasks })
  assert.ok(Array.isArray(prepared.tasks), 'legacy path turns the string into an array')
})

test('Kernel live-loop tool forwards a JSON-string task list unchanged (no preparer)', () => {
  const session = { isIdle: true, agent: { state: { tools: [] }, subscribe() {}, abort() {} } }
  installKernelHostTools(session, [{
    name: 'plan_revision_create',
    description: 'plan revision under test',
    parameters: { type: 'object', properties: { tasks: { type: 'array' } }, required: ['tasks'] },
  }], { settledFor: () => ({ content: [], isError: false }) })
  const [tool] = session.agent.state.tools
  assert.equal(tool.name, 'plan_revision_create')
  assert.equal(tool.prepareArguments, undefined,
    'no argument preparation on the Kernel path: the Host receives tasks as a string')
  assert.deepEqual(tool.parameters, { type: 'object' },
    'engine-side validation is permissive, so the string is not rejected before the Host')
})
