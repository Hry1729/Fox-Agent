import test from 'node:test'
import assert from 'node:assert/strict'
import { Check } from 'typebox/value'
import {
  createHostTools,
  createKnowledgeTools,
  KNOWLEDGE_REFERENCE_SCHEMA,
} from '../src/host-tools.mjs'
import { diagnoseToolsForAgentContext } from '../src/expert-package.mjs'
import {
  KNOWLEDGE_TOOL_NAMES,
  runtimeToolNames,
} from '../src/runtime-contract.mjs'
import { VALIDATION_CHECK_TYPES } from '../src/validation-policy.mjs'

function toolByName(tools, name) {
  const tool = tools.find((entry) => entry.name === name)
  assert.ok(tool, `missing tool ${name}`)
  return tool
}

function objectProperties(schema) {
  assert.equal(schema.type, 'object')
  return schema.properties
}

test('knowledge tools normalize serialized references before strict schema validation', () => {
  const reference = { source: 'remote', connectionId: 'yuxi-primary', id: 'kb-crane' }
  const search = toolByName(createKnowledgeTools(() => {}), KNOWLEDGE_TOOL_NAMES.search)
  const input = { query: '起升 赋值', target: JSON.stringify(reference) }
  const prepared = search.prepareArguments(input)
  assert.deepEqual(prepared, { query: '起升 赋值', target: reference })
  assert.equal(Check(search.parameters, prepared), true)
  assert.equal(typeof input.target, 'string')
  assert.equal(Check(search.parameters, search.prepareArguments({ query: '起升', targets: JSON.stringify([reference]) })), true)
  for (const target of ['{bad json}', '[]', 'null', '{"source":"remote","id":"kb-crane"}']) {
    assert.equal(Check(search.parameters, search.prepareArguments({ query: '起升', target })), false)
  }
  const read = toolByName(createKnowledgeTools(() => {}), KNOWLEDGE_TOOL_NAMES.read)
  assert.equal(Check(read.parameters, read.prepareArguments({ target: JSON.stringify(reference), documentId: 'file-1' })), true)
})

const CAPABILITY_TOOL_NAMES = [
  'web_search',
  'web_read',
  'http_request',
  'system_info',
  'sqlite_read',
  'structured_data',
  'git_read',
  'test_run',
  'code_check',
  'format_code',
  'tabular_data',
]

test('registers the expanded Host capability tools in catalog order', () => {
  const names = createHostTools(() => {}).map(({ name }) => name)
  const runCommandIndex = names.indexOf('run_command')

  assert.deepEqual(
    names.slice(runCommandIndex + 1, runCommandIndex + 1 + CAPABILITY_TOOL_NAMES.length),
    CAPABILITY_TOOL_NAMES,
  )
  assert.deepEqual(
    runtimeToolNames().filter((name) => CAPABILITY_TOOL_NAMES.includes(name)),
    CAPABILITY_TOOL_NAMES,
  )
})

test('publishes bounded schemas for the expanded Host capability tools', () => {
  const tools = createHostTools(() => {})

  const webSearch = objectProperties(toolByName(tools, 'web_search').parameters)
  assert.equal(webSearch.query.type, 'string')
  assert.equal(webSearch.maxResults.minimum, 1)
  assert.equal(webSearch.maxResults.maximum, 10)
  assert.deepEqual(
    webSearch.provider.anyOf.map(({ const: value }) => value),
    ['auto', 'brave', 'tavily', 'exa', 'searxng', 'duckduckgo'],
  )

  const webRead = objectProperties(toolByName(tools, 'web_read').parameters)
  assert.equal(webRead.url.type, 'string')
  assert.equal(webRead.maxChars.maximum, 300000)

  const http = objectProperties(toolByName(tools, 'http_request').parameters)
  assert.equal(http.allowedHosts.maxItems, 20)
  assert.equal(http.timeoutSeconds.maximum, 15)

  const system = objectProperties(toolByName(tools, 'system_info').parameters)
  assert.equal(system.maxProcesses.maximum, 100)

  const sqlite = objectProperties(toolByName(tools, 'sqlite_read').parameters)
  assert.equal(sqlite.rowLimit.maximum, 1000)
  assert.equal(sqlite.timeoutSeconds.maximum, 5)

  const structured = objectProperties(toolByName(tools, 'structured_data').parameters)
  assert.equal(structured.action.anyOf.length, 4)
  assert.equal(structured.requiredKeys.type, 'array')

  const git = objectProperties(toolByName(tools, 'git_read').parameters)
  assert.equal(git.operation.anyOf.length, 4)
  assert.equal(git.maxEntries.maximum, 100)

  const testRun = objectProperties(toolByName(tools, 'test_run').parameters)
  assert.equal(testRun.timeoutSeconds.maximum, 600)

  const format = objectProperties(toolByName(tools, 'format_code').parameters)
  assert.deepEqual(format.mode.anyOf.map(({ const: value }) => value), ['check', 'write'])

  const table = objectProperties(toolByName(tools, 'tabular_data').parameters)
  assert.equal(table.limit.maximum, 500)
  assert.equal(table.aggregate.anyOf.length, 5)
})

test('forwards expanded Host capability calls without transforming inputs', async () => {
  const requests = []
  const requestHost = async (type, payload) => {
    requests.push({ type, payload })
    return { payload: { result: { ok: true } } }
  }
  const tools = createHostTools(requestHost)
  const cases = [
    ['web_search', { query: 'Fox Agent', maxResults: 3 }],
    ['web_read', { url: 'https://example.com', format: 'markdown' }],
    ['http_request', { url: 'https://example.com/api', allowedHosts: ['example.com'] }],
    ['system_info', { includeProcesses: false }],
    ['sqlite_read', { path: 'data.sqlite', query: 'SELECT 1' }],
    ['structured_data', { action: 'query', data: '{"fox":true}', query: 'fox' }],
    ['git_read', { operation: 'status' }],
    ['test_run', { runner: 'rust', timeoutSeconds: 30 }],
    ['code_check', { check: 'lint', ecosystem: 'node' }],
    ['format_code', { mode: 'check', ecosystem: 'python' }],
    ['tabular_data', { operation: 'preview', data: 'name\nFox', format: 'csv' }],
  ]

  for (const [name, input] of cases) {
    await toolByName(tools, name).execute(`call-${name}`, input)
  }

  assert.deepEqual(requests, cases.map(([name, input]) => ({
    type: 'tool.execute',
    payload: { toolCallId: `call-${name}`, tool: name, input },
  })))
})

test('publishes backward-compatible serial and strict bounded Graph PlanRevision schemas', () => {
  const tool = toolByName(createHostTools(() => {}), 'plan_revision_create')
  const schema = tool.parameters
  assert.equal(schema.additionalProperties, false)
  assert.equal(schema.properties.tasks.anyOf.length, 2)
  assert.equal(typeof tool.prepareArguments, 'function')

  const serial = {
    goalId: 'goal-1',
    title: 'Serial plan',
    summary: 'Keep the existing ordered plan shape.',
    tasks: [{ title: 'Implement', detail: null, ordinal: 0 }],
  }
  const graph = {
    goalId: 'goal-1',
    title: 'Read-only Graph plan',
    summary: 'One root and two bounded readers.',
    tasks: [
      { nodeKey: 'root', title: 'Root', ordinal: 0, acceptanceCriteria: ['root report is non-empty'] },
      { nodeKey: 'left', title: 'Left', ordinal: 1, dependsOn: ['root'], acceptanceCriteria: ['left report inspected'] },
      { nodeKey: 'right', title: 'Right', ordinal: 2, dependsOn: ['root'], acceptanceCriteria: ['right report inspected'] },
    ],
  }
  assert.equal(Check(schema, serial), true)
  assert.equal(Check(schema, graph), true)
  const stringifiedTasks = tool.prepareArguments({ ...serial, tasks: JSON.stringify(serial.tasks) })
  assert.deepEqual(stringifiedTasks.tasks, serial.tasks)
  assert.equal(Check(schema, stringifiedTasks), true)
  const malformedTasks = tool.prepareArguments({ ...serial, tasks: '[{' })
  assert.equal(typeof malformedTasks.tasks, 'string')
  assert.equal(Check(schema, malformedTasks), false)
  assert.equal(Check(schema, { ...serial, tasks: [{ title: 'x'.repeat(500), ordinal: 0 }] }), true)
  assert.equal(Check(schema, { ...serial, tasks: [{ title: 'x'.repeat(501), ordinal: 0 }] }), false)
  assert.equal(Check(schema, { ...graph, tasks: [{ ...graph.tasks[0], title: 'x'.repeat(300) }] }), true)
  assert.equal(Check(schema, { ...graph, tasks: [{ ...graph.tasks[0], title: 'x'.repeat(301) }] }), false)

  const invalid = [
    { ...serial, tasks: [{ title: 'Implement', ordinal: 0, status: 'completed' }] },
    { ...graph, tasks: graph.tasks.map(({ acceptanceCriteria: _criteria, ...task }) => task) },
    { ...graph, tasks: [...graph.tasks, { nodeKey: 'fourth', title: 'Fourth', ordinal: 3, acceptanceCriteria: ['fourth'] }] },
    { ...graph, tasks: [{ ...graph.tasks[0], nodeKey: 'bad key' }] },
    { ...graph, tasks: [{ ...graph.tasks[0], acceptanceCriteria: [] }] },
    { ...graph, tasks: [{ ...graph.tasks[0], acceptanceCriteria: Array.from({ length: 9 }, (_, index) => `criterion-${index}`) }] },
    { ...graph, tasks: [{ ...graph.tasks[0], acceptanceCriteria: ['duplicate', 'duplicate'] }] },
    { ...graph, tasks: [{ ...graph.tasks[0], dependsOn: ['left', 'left'] }] },
    { ...graph, tasks: [{ ...graph.tasks[0], title: ' Root' }] },
    { ...graph, tasks: [{ ...graph.tasks[0], acceptanceCriteria: ['\u00a0criterion'] }] },
    { ...graph, runId: 'model-controlled' },
  ]
  for (const value of invalid) assert.equal(Check(schema, value), false, JSON.stringify(value))
})

test('publishes and forwards only bounded persistent read-only Graph inputs', async () => {
  const requests = []
  const requestHost = async (type, payload) => {
    requests.push({ type, payload })
    return { payload: { result: { activated: false, graph: null } } }
  }
  const tools = createHostTools(requestHost)
  const activate = toolByName(tools, 'graph_readonly_activate')
  const snapshot = toolByName(tools, 'graph_readonly_snapshot_get')
  const start = toolByName(tools, 'graph_readonly_node_start')
  const review = toolByName(tools, 'graph_readonly_node_review')
  const finish = toolByName(tools, 'graph_readonly_node_finish')
  const cancel = toolByName(tools, 'graph_readonly_node_cancel')
  const accept = toolByName(tools, 'graph_readonly_accept')
  const nodeStart = {
    goalId: 'goal-1',
    taskId: 'task-1',
    expectedTaskVersion: 1,
    attemptId: 'attempt-1',
    workerAgentId: 'fox-general',
    objective: 'Inspect the bounded node inputs.',
    context: 'Read only the authorized project files.',
    budget: {
      maxDurationMs: 45000,
      maxTotalTokens: 4096,
      maxOutputTokens: 1024,
      maxToolCalls: 6,
    },
  }
  const nodeFinish = {
    goalId: 'goal-1',
    taskId: 'task-1',
    attemptId: 'attempt-1',
    expectedTaskVersion: 2,
    expectedAttemptVersion: 1,
    criterionEvidence: [
      { criterion: 'The inspection is complete.', evidenceIds: ['evidence-1'] },
    ],
    summary: 'The parent Lead inspected and validated the delegated result.',
  }
  const nodeCancel = {
    goalId: 'goal-1',
    taskId: 'task-1',
    attemptId: 'attempt-1',
    expectedTaskVersion: 2,
    expectedAttemptVersion: 1,
    reason: 'The bounded inspection is no longer required.',
  }
  const graphAccept = {
    goalId: 'goal-1',
    expectedGoalVersion: 3,
    summary: 'Every node is accepted under the current Plan with no open findings.',
  }
  assert.match(review.description, /running high-risk read-only Graph Attempt whose implementation Child completed/)
  assert.doesNotMatch(review.description, /completed high-risk read-only Graph Attempt/)
  assert.match(review.description, /later node finish must reuse this review request unchanged.*exact criterionEvidence and summary/)
  assert.match(finish.description, /high-risk reviewed node.*passed review request unchanged.*exact criterionEvidence and summary/)

  assert.equal(Check(activate.parameters, { planRevisionId: 'plan-1' }), true)
  assert.equal(Check(activate.parameters, { planRevisionId: ' plan-1' }), false)
  assert.equal(Check(activate.parameters, { planRevisionId: 'plan-1', nodes: [] }), false)
  assert.equal(Check(snapshot.parameters, { goalId: 'goal-1' }), true)
  assert.equal(Check(snapshot.parameters, { goalId: '' }), false)
  assert.equal(Check(snapshot.parameters, { goalId: 'goal-1', start: true }), false)
  assert.equal(Check(start.parameters, nodeStart), true)
  assert.equal(Check(start.parameters, { ...nodeStart, allowedTools: ['write_file'] }), false)
  assert.equal(Check(start.parameters, { ...nodeStart, executionProfile: 'legacy' }), false)
  assert.equal(Check(start.parameters, { ...nodeStart, profile: 'durable_v2' }), false)
  assert.equal(Check(start.parameters, { ...nodeStart, authority: 'host' }), false)
  assert.equal(Check(start.parameters, { ...nodeStart, runId: 'model-controlled' }), false)
  assert.equal(Check(start.parameters, { ...nodeStart, toolCallId: 'model-controlled' }), false)
  assert.equal(Check(start.parameters, { ...nodeStart, expectedTaskVersion: 0 }), false)
  assert.equal(Check(start.parameters, { ...nodeStart, context: ' context' }), false)
  assert.equal(Check(start.parameters, {
    ...nodeStart,
    budget: { ...nodeStart.budget, maxDurationMs: 45001 },
  }), false)
  assert.equal(Check(start.parameters, {
    ...nodeStart,
    budget: { ...nodeStart.budget, maxToolCalls: 7 },
  }), false)
  assert.equal(Check(finish.parameters, nodeFinish), true)
  assert.equal(Check(finish.parameters, { ...nodeFinish, runId: 'model-controlled' }), false)
  assert.equal(Check(finish.parameters, { ...nodeFinish, childRunId: 'model-controlled' }), false)
  assert.equal(Check(finish.parameters, { ...nodeFinish, validationPolicyId: 'model-controlled' }), false)
  assert.equal(Check(finish.parameters, { ...nodeFinish, authority: 'host' }), false)
  assert.equal(Check(finish.parameters, { ...nodeFinish, criterionEvidence: [] }), false)
  assert.equal(Check(finish.parameters, {
    ...nodeFinish,
    criterionEvidence: [{ criterion: 'The inspection is complete.', evidenceIds: [] }],
  }), false)
  assert.equal(Check(finish.parameters, {
    ...nodeFinish,
    criterionEvidence: [{ criterion: 'The inspection is complete.', evidenceIds: ['same', 'same'] }],
  }), false)
  assert.deepEqual(review.parameters, finish.parameters)
  assert.equal(Check(review.parameters, nodeFinish), true)
  assert.equal(Check(review.parameters, { ...nodeFinish, reviewerRunId: 'model-controlled' }), false)
  assert.equal(Check(review.parameters, { ...nodeFinish, reviewerKind: 'lead' }), false)
  assert.equal(Check(review.parameters, { ...nodeFinish, reviewOutcome: 'pass' }), false)
  assert.equal(Check(review.parameters, { ...nodeFinish, criterionEvidence: [] }), false)
  assert.equal(Check(cancel.parameters, nodeCancel), true)
  assert.equal(Check(cancel.parameters, { ...nodeCancel, childRunId: 'model-controlled' }), false)
  assert.equal(Check(cancel.parameters, { ...nodeCancel, parentRunId: 'model-controlled' }), false)
  assert.equal(Check(cancel.parameters, { ...nodeCancel, status: 'cancelled' }), false)
  assert.equal(Check(cancel.parameters, { ...nodeCancel, authority: 'host' }), false)
  assert.equal(Check(cancel.parameters, { ...nodeCancel, reason: ' reason' }), false)
  assert.equal(Check(cancel.parameters, { ...nodeCancel, expectedAttemptVersion: 0 }), false)
  assert.equal(Check(accept.parameters, graphAccept), true)
  assert.deepEqual(Object.keys(accept.parameters.properties), ['goalId', 'expectedGoalVersion', 'summary'])
  assert.deepEqual(accept.parameters.required, ['goalId', 'expectedGoalVersion', 'summary'])
  assert.equal(accept.parameters.additionalProperties, false)
  assert.equal(Check(accept.parameters, { ...graphAccept, planRevisionId: 'model-controlled' }), false)
  assert.equal(Check(accept.parameters, { ...graphAccept, reviewerRunId: 'model-controlled' }), false)
  assert.equal(Check(accept.parameters, { ...graphAccept, expectedGoalVersion: 0 }), false)
  assert.equal(Check(accept.parameters, { ...graphAccept, summary: ' summary' }), false)

  await activate.execute('activate-call', { planRevisionId: 'plan-1' })
  await snapshot.execute('snapshot-call', { goalId: 'goal-1' })
  await start.execute('node-start-call', nodeStart)
  await review.execute('node-review-call', nodeFinish)
  await finish.execute('node-finish-call', nodeFinish)
  await cancel.execute('node-cancel-call', nodeCancel)
  await accept.execute('graph-accept-call', graphAccept)
  assert.deepEqual(requests, [
    {
      type: 'tool.execute',
      payload: {
        toolCallId: 'activate-call',
        tool: 'graph_readonly_activate',
        input: { planRevisionId: 'plan-1' },
      },
    },
    {
      type: 'tool.execute',
      payload: {
        toolCallId: 'snapshot-call',
        tool: 'graph_readonly_snapshot_get',
        input: { goalId: 'goal-1' },
      },
    },
    {
      type: 'tool.execute',
      payload: {
        toolCallId: 'node-start-call',
        tool: 'graph_readonly_node_start',
        input: nodeStart,
      },
    },
    {
      type: 'tool.execute',
      payload: {
        toolCallId: 'node-review-call',
        tool: 'graph_readonly_node_review',
        input: nodeFinish,
      },
    },
    {
      type: 'tool.execute',
      payload: {
        toolCallId: 'node-finish-call',
        tool: 'graph_readonly_node_finish',
        input: nodeFinish,
      },
    },
    {
      type: 'tool.execute',
      payload: {
        toolCallId: 'node-cancel-call',
        tool: 'graph_readonly_node_cancel',
        input: nodeCancel,
      },
    },
    {
      type: 'tool.execute',
      payload: {
        toolCallId: 'graph-accept-call',
        tool: 'graph_readonly_accept',
        input: graphAccept,
      },
    },
  ])
})

test('publishes fail-closed schemas for task attempts, repair, risk, and validation checks', () => {
  const tools = createHostTools(() => {})
  const attemptStart = toolByName(tools, 'task_attempt_start').parameters
  const repairStart = toolByName(tools, 'task_repair_start').parameters
  const repairEscalateStart = toolByName(tools, 'task_repair_escalate_start').parameters
  const attemptFinish = toolByName(tools, 'task_attempt_finish').parameters
  const taskCreateMany = toolByName(tools, 'task_create_many').parameters
  const evidenceAdd = toolByName(tools, 'task_evidence_add').parameters

  assert.deepEqual(
    runtimeToolNames().filter((name) => [
      'task_attempt_start',
      'task_repair_start',
      'task_repair_escalate_start',
      'task_attempt_finish',
    ].includes(name)),
    ['task_attempt_start', 'task_repair_start', 'task_repair_escalate_start', 'task_attempt_finish'],
  )
  assert.equal(attemptStart.additionalProperties, false)
  assert.equal(attemptStart.properties.taskId.maxLength, 200)
  assert.equal(attemptStart.properties.attemptId.maxLength, 200)
  assert.equal(attemptStart.properties.expectedVersion.minimum, 1)
  assert.equal(Check(attemptStart, {
    taskId: 'task-1', attemptId: 'attempt-1', expectedVersion: 1,
  }), true)
  assert.equal(Check(attemptStart, {
    taskId: 'task-1', attemptId: 'attempt-1', expectedVersion: 1, runId: 'model-run',
  }), false)
  assert.equal(Check(attemptStart, {
    taskId: 'x'.repeat(201), attemptId: 'attempt-1', expectedVersion: 1,
  }), false)

  assert.equal(repairStart.additionalProperties, false)
  assert.equal(repairStart.properties.rootCause.minLength, 1)
  assert.equal(repairStart.properties.rootCause.maxLength, 4000)
  assert.equal(repairStart.properties.findingIds.minItems, 1)
  assert.equal(repairStart.properties.findingIds.maxItems, 32)
  assert.equal(repairStart.properties.findingIds.uniqueItems, true)
  assert.equal(repairStart.properties.findingIds.items.minLength, 1)
  assert.equal(repairStart.properties.findingIds.items.maxLength, 200)
  const validRepair = {
    taskId: 'task-1',
    attemptId: 'repair-1',
    expectedVersion: 2,
    rootCause: 'Validation failed after an API change.',
    findingIds: ['finding-1'],
  }
  assert.equal(Check(repairStart, validRepair), true)
  for (const field of ['taskId', 'attemptId', 'rootCause']) {
    assert.match(repairStart.properties[field].pattern, /\\s/)
    for (const invalid of ['', '   ', ' leading', 'trailing ', '\u00a0unicode-leading', 'unicode-trailing\u2028', '\u0085rust-trim-leading', 'rust-trim-trailing\u0085']) {
      assert.equal(Check(repairStart, { ...validRepair, [field]: invalid }), false, `${field}: ${JSON.stringify(invalid)}`)
    }
    for (const valid of ['internal space', 'internal\nline']) {
      assert.equal(Check(repairStart, { ...validRepair, [field]: valid }), true, `${field}: ${JSON.stringify(valid)}`)
    }
  }
  assert.match(repairStart.properties.findingIds.items.pattern, /\\s/)
  assert.match(repairStart.properties.findingIds.items.pattern, /\\u0085/)
  for (const invalid of ['', '   ', ' leading', 'trailing ', '\u00a0unicode-leading', 'unicode-trailing\u2028', '\u0085rust-trim-leading', 'rust-trim-trailing\u0085']) {
    assert.equal(Check(repairStart, { ...validRepair, findingIds: [invalid] }), false, `findingIds: ${JSON.stringify(invalid)}`)
  }
  for (const valid of ['internal space', 'internal\nline']) {
    assert.equal(Check(repairStart, { ...validRepair, findingIds: [valid] }), true, `findingIds: ${JSON.stringify(valid)}`)
  }
  assert.equal(Check(repairStart, {
    taskId: 'task-1',
    attemptId: 'repair-1',
    expectedVersion: 2,
    rootCause: 'duplicate findings',
    findingIds: ['finding-1', 'finding-1'],
  }), false)

  assert.equal(repairEscalateStart.additionalProperties, false)
  assert.deepEqual(Object.keys(repairEscalateStart.properties), [
    'taskId',
    'attemptId',
    'expectedVersion',
    'rootCause',
    'findingIds',
    'escalationReason',
  ])
  assert.deepEqual(repairEscalateStart.required, [
    'taskId',
    'attemptId',
    'expectedVersion',
    'rootCause',
    'findingIds',
    'escalationReason',
  ])
  assert.equal(repairEscalateStart.properties.taskId.minLength, 1)
  assert.equal(repairEscalateStart.properties.taskId.maxLength, 200)
  assert.equal(repairEscalateStart.properties.attemptId.minLength, 1)
  assert.equal(repairEscalateStart.properties.attemptId.maxLength, 200)
  assert.equal(repairEscalateStart.properties.expectedVersion.minimum, 1)
  assert.equal(repairEscalateStart.properties.rootCause.minLength, 1)
  assert.equal(repairEscalateStart.properties.rootCause.maxLength, 4000)
  assert.equal(repairEscalateStart.properties.findingIds.minItems, 1)
  assert.equal(repairEscalateStart.properties.findingIds.maxItems, 32)
  assert.equal(repairEscalateStart.properties.findingIds.uniqueItems, true)
  assert.equal(repairEscalateStart.properties.findingIds.items.minLength, 1)
  assert.equal(repairEscalateStart.properties.findingIds.items.maxLength, 200)
  assert.equal(repairEscalateStart.properties.escalationReason.minLength, 1)
  assert.equal(repairEscalateStart.properties.escalationReason.maxLength, 2000)
  const validEscalation = {
    taskId: 'task-1',
    attemptId: 'repair-human-1',
    expectedVersion: 3,
    rootCause: 'Automated repair attempts exhausted the frozen budget.',
    findingIds: ['finding-1', 'finding-2'],
    escalationReason: 'A human must authorize the next repair attempt.',
  }
  assert.equal(Check(repairEscalateStart, validEscalation), true)
  for (const field of ['taskId', 'attemptId', 'rootCause', 'escalationReason']) {
    assert.match(repairEscalateStart.properties[field].pattern, /\\s/)
    for (const invalid of ['', '   ', ' leading', 'trailing ', '\u00a0unicode-leading', 'unicode-trailing\u2028', '\u0085rust-trim-leading', 'rust-trim-trailing\u0085']) {
      assert.equal(Check(repairEscalateStart, { ...validEscalation, [field]: invalid }), false, `${field}: ${JSON.stringify(invalid)}`)
    }
    for (const valid of ['internal space', 'internal\nline']) {
      assert.equal(Check(repairEscalateStart, { ...validEscalation, [field]: valid }), true, `${field}: ${JSON.stringify(valid)}`)
    }
  }
  assert.match(repairEscalateStart.properties.findingIds.items.pattern, /\\s/)
  assert.match(repairEscalateStart.properties.findingIds.items.pattern, /\\u0085/)
  for (const invalid of ['', '   ', ' leading', 'trailing ', '\u00a0unicode-leading', 'unicode-trailing\u2028', '\u0085rust-trim-leading', 'rust-trim-trailing\u0085']) {
    assert.equal(Check(repairEscalateStart, { ...validEscalation, findingIds: [invalid] }), false, `findingIds: ${JSON.stringify(invalid)}`)
  }
  for (const valid of ['internal space', 'internal\nline']) {
    assert.equal(Check(repairEscalateStart, { ...validEscalation, findingIds: [valid] }), true, `findingIds: ${JSON.stringify(valid)}`)
  }
  const { escalationReason: _omittedEscalationReason, ...withoutEscalationReason } = validEscalation
  assert.equal(Check(repairEscalateStart, withoutEscalationReason), false)
  for (const forbiddenField of ['runId', 'conversationId', 'approval', 'policy', 'grant', 'count']) {
    assert.equal(Check(repairEscalateStart, {
      ...validEscalation,
      [forbiddenField]: forbiddenField === 'count' ? 1 : 'model-controlled',
    }), false, forbiddenField)
  }
  assert.equal(Check(repairEscalateStart, { ...validEscalation, expectedVersion: 0 }), false)
  assert.equal(Check(repairEscalateStart, { ...validEscalation, findingIds: [] }), false)
  assert.equal(Check(repairEscalateStart, { ...validEscalation, findingIds: ['finding-1', 'finding-1'] }), false)
  assert.equal(Check(repairEscalateStart, { ...validEscalation, escalationReason: 'x'.repeat(2001) }), false)

  assert.equal(attemptFinish.additionalProperties, false)
  assert.equal(attemptFinish.properties.expectedAttemptVersion.minimum, 1)
  assert.deepEqual(
    attemptFinish.properties.status.anyOf.map(({ const: value }) => value),
    ['succeeded', 'failed', 'blocked', 'cancelled'],
  )
  assert.equal(Check(attemptFinish, {
    taskId: 'task-1',
    attemptId: 'attempt-1',
    expectedVersion: 3,
    expectedAttemptVersion: 1,
    status: 'succeeded',
  }), true)
  assert.equal(Check(attemptFinish, {
    taskId: 'task-1',
    attemptId: 'attempt-1',
    expectedVersion: 3,
    expectedAttemptVersion: 1,
    status: 'succeeded',
    failureReason: 'must not be present',
  }), false)
  assert.equal(Check(attemptFinish, {
    taskId: 'task-1',
    attemptId: 'attempt-1',
    expectedVersion: 3,
    expectedAttemptVersion: 1,
    status: 'failed',
  }), false)
  assert.equal(Check(attemptFinish, {
    taskId: 'task-1',
    attemptId: 'attempt-1',
    expectedVersion: 3,
    expectedAttemptVersion: 1,
    status: 'blocked',
    failureReason: 'External dependency is unavailable.',
  }), true)

  const taskItem = taskCreateMany.properties.tasks.items
  assert.equal(taskCreateMany.additionalProperties, false)
  assert.equal(taskItem.additionalProperties, false)
  assert.deepEqual(
    taskItem.properties.riskLevel.anyOf.map(({ const: value }) => value),
    ['low', 'standard', 'high', 'critical'],
  )
  assert.equal(Check(taskCreateMany, {
    goalId: 'goal-1',
    tasks: [{ title: 'Verify change', ordinal: 0, riskLevel: 'critical' }],
  }), true)
  assert.equal(Check(taskCreateMany, {
    goalId: 'goal-1',
    tasks: [{ title: 'Verify change', ordinal: 0, riskLevel: 'none' }],
  }), false)

  assert.equal(evidenceAdd.additionalProperties, false)
  assert.deepEqual(
    evidenceAdd.properties.validationCheckType.anyOf.map(({ const: value }) => value),
    VALIDATION_CHECK_TYPES,
  )
  const validEvidence = {
    taskId: 'task-1',
    evidenceType: 'test_result',
    refKind: 'tool_call',
    refId: 'tool-1',
    summary: 'Focused tests passed.',
    validationCheckType: 'test',
  }
  assert.equal(Check(evidenceAdd, validEvidence), true)
  assert.equal(Check(evidenceAdd, { ...validEvidence, validationCheckType: 'shell' }), false)
  assert.equal(Check(evidenceAdd, { ...validEvidence, policy: 'model-policy' }), false)
})

test('forwards task attempt and validation inputs unchanged without authority fields', async () => {
  const requests = []
  const requestHost = async (type, payload) => {
    requests.push({ type, payload })
    return { payload: { result: { ok: true } } }
  }
  const tools = createHostTools(requestHost)
  const cases = [
    ['task_attempt_start', {
      taskId: 'task-1', attemptId: 'attempt-1', expectedVersion: 2,
    }],
    ['task_repair_start', {
      taskId: 'task-1',
      attemptId: 'repair-1',
      expectedVersion: 3,
      rootCause: 'The first validation found a stale fixture.',
      findingIds: ['finding-1', 'finding-2'],
    }],
    ['task_repair_escalate_start', {
      taskId: 'task-1',
      attemptId: 'repair-human-1',
      expectedVersion: 4,
      rootCause: 'Automated repairs cannot safely choose the external migration.',
      findingIds: ['finding-3'],
      escalationReason: 'A human must approve the migration-specific repair.',
    }],
    ['task_attempt_finish', {
      taskId: 'task-1',
      attemptId: 'repair-1',
      expectedVersion: 4,
      expectedAttemptVersion: 1,
      status: 'failed',
      failureReason: 'The repaired fixture still fails.',
    }],
    ['task_create_many', {
      goalId: 'goal-1',
      tasks: [{ title: 'Verify authorization', ordinal: 0, riskLevel: 'high' }],
    }],
    ['task_evidence_add', {
      taskId: 'task-1',
      evidenceType: 'test_result',
      refKind: 'tool_call',
      refId: 'tool-1',
      summary: 'Authorization tests passed.',
      validationCheckType: 'test',
    }],
  ]

  for (const [name, input] of cases) {
    await toolByName(tools, name).execute(`call-${name}`, input)
  }

  assert.deepEqual(requests, cases.map(([name, input]) => ({
    type: 'tool.execute',
    payload: { toolCallId: `call-${name}`, tool: name, input },
  })))
  for (const { payload } of requests) {
    assert.equal('conversationId' in payload.input, false)
    assert.equal('runId' in payload.input, false)
    assert.equal('policy' in payload.input, false)
    assert.equal('approval' in payload.input, false)
    assert.equal('grant' in payload.input, false)
    assert.equal('count' in payload.input, false)
  }
})

test('publishes bounded Child Run schemas in delegation catalog order', () => {
  const tools = createHostTools(() => {})
  const delegationNames = [
    'child_agent_list',
    'child_run_start',
    'child_run_collect',
    'child_run_cancel',
  ]
  assert.deepEqual(
    tools.filter(({ name }) => delegationNames.includes(name)).map(({ name }) => name),
    delegationNames,
  )
  assert.deepEqual(
    runtimeToolNames().filter((name) => delegationNames.includes(name)),
    delegationNames,
  )

  const start = objectProperties(toolByName(tools, 'child_run_start').parameters)
  assert.deepEqual(start.mode.anyOf.map(({ const: value }) => value), ['worker', 'expert_consultation'])
  assert.equal(start.objective.maxLength, 8000)
  assert.equal(start.context.maxLength, 12000)
  assert.equal(start.agentId.maxLength, 160)
  assert.equal(start.expertId.maxLength, 160)
  assert.equal(start.budget.properties.maxDurationMs.maximum, 900000)
  assert.deepEqual(Object.keys(start.budget.properties), ['maxDurationMs'])

  const collect = objectProperties(toolByName(tools, 'child_run_collect').parameters)
  assert.equal(collect.childRunIds.minItems, 1)
  assert.equal(collect.childRunIds.maxItems, 8)
  assert.equal(collect.childRunIds.items.minLength, 1)
  assert.equal(collect.childRunIds.items.maxLength, 160)
  assert.match(objectProperties(toolByName(tools, 'child_run_start').parameters).objective.description, /exact key: objective/)
  assert.equal(toolByName(tools, 'child_run_start').parameters.additionalProperties, false)
  assert.match(toolByName(tools, 'child_run_collect').description, /never invent IDs/)
  assert.equal(collect.waitMs.maximum, 60000)
})

test('forwards Child Run orchestration inputs without changing authority fields', async () => {
  const requests = []
  const requestHost = async (type, payload) => {
    requests.push({ type, payload })
    return { payload: { result: { ok: true } } }
  }
  const tools = createHostTools(requestHost)
  const cases = [
    ['child_agent_list', {}],
    ['child_run_start', {
      mode: 'worker',
      objective: 'Review the cancellation path',
      context: 'Inspect only the supplied run IDs.',
      agentId: 'fox-general',
      budget: { maxDurationMs: 30000 },
    }],
    ['child_run_start', {
      mode: 'expert_consultation',
      expertId: 'fox-reviewer',
      objective: 'Review the proposed boundary',
      context: 'Return advice to the lead without changing the attached expert.',
      budget: { maxDurationMs: 30000 },
    }],
    ['child_run_collect', { childRunIds: ['child-1', 'child-2'], waitMs: 1000 }],
    ['child_run_cancel', { childRunId: 'child-2' }],
  ]

  for (const [name, input] of cases) {
    await toolByName(tools, name).execute(`call-${name}`, input)
  }

  assert.deepEqual(requests, cases.map(([name, input]) => ({
    type: 'tool.execute',
    payload: { toolCallId: `call-${name}`, tool: name, input },
  })))
})

test('publishes and forwards bounded serial expert Team tools', async () => {
  const requests = []
  const requestHost = async (type, payload) => {
    requests.push({ type, payload })
    return { payload: { result: { ok: true } } }
  }
  const tools = createHostTools(requestHost)
  const names = ['team_snapshot_get', 'team_start', 'team_member_start', 'team_collect', 'team_cancel']
  assert.deepEqual(
    tools.filter(({ name }) => names.includes(name)).map(({ name }) => name),
    names,
  )
  assert.deepEqual(runtimeToolNames().filter((name) => names.includes(name)), names)
  const start = objectProperties(toolByName(tools, 'team_start').parameters)
  assert.equal(start.objective.maxLength, 8000)
  assert.equal(start.context.maxLength, 12000)
  const member = objectProperties(toolByName(tools, 'team_member_start').parameters)
  assert.equal(member.memberId.maxLength, 128)
  assert.equal(member.task.maxLength, 8000)
  assert.equal(member.context.maxLength, 12000)
  const collect = objectProperties(toolByName(tools, 'team_collect').parameters)
  assert.equal(collect.waitMs.maximum, 60000)

  const cases = [
    ['team_snapshot_get', {}],
    ['team_start', { objective: 'Deliver a verified change', context: 'Only the named project.' }],
    ['team_member_start', { memberId: 'reviewer', task: 'Review the change', context: 'Inspect the final diff.' }],
    ['team_collect', { waitMs: 1000 }],
    ['team_cancel', { reason: 'User stopped the team' }],
  ]
  for (const [name, input] of cases) {
    await toolByName(tools, name).execute(`call-${name}`, input)
  }
  assert.deepEqual(requests, cases.map(([name, input]) => ({
    type: 'tool.execute',
    payload: { toolCallId: `call-${name}`, tool: name, input },
  })))
})

test('publishes governed memory tools and forwards candidate evidence unchanged', async () => {
  const requests = []
  const requestHost = async (type, payload) => {
    requests.push({ type, payload })
    return { payload: { result: { status: 'candidate' } } }
  }
  const tools = createHostTools(requestHost)
  const search = toolByName(tools, 'memory_search')
  const propose = toolByName(tools, 'memory_propose')
  const searchSchema = objectProperties(search.parameters)
  const proposalSchema = objectProperties(propose.parameters)

  assert.equal(searchSchema.limit.maximum, 20)
  assert.equal(proposalSchema.content.maxLength, 4000)
  assert.equal(proposalSchema.evidenceExcerpt.maxLength, 1000)
  assert.deepEqual(
    runtimeToolNames().filter((name) => name.startsWith('memory_')),
    ['memory_search', 'memory_propose'],
  )

  await search.execute('memory-search-1', { query: 'preferred editor', limit: 4 })
  await propose.execute('memory-propose-1', {
    scope: 'agent',
    kind: 'preference',
    canonicalKey: 'preferred_editor',
    content: 'The user prefers Vim.',
    evidenceExcerpt: 'The user explicitly said to use Vim.',
    confidence: 0.9,
  })

  assert.deepEqual(requests, [
    {
      type: 'tool.execute',
      payload: {
        toolCallId: 'memory-search-1',
        tool: 'memory_search',
        input: { query: 'preferred editor', limit: 4 },
      },
    },
    {
      type: 'tool.execute',
      payload: {
        toolCallId: 'memory-propose-1',
        tool: 'memory_propose',
        input: {
          scope: 'agent',
          kind: 'preference',
          canonicalKey: 'preferred_editor',
          content: 'The user prefers Vim.',
          evidenceExcerpt: 'The user explicitly said to use Vim.',
          confidence: 0.9,
        },
      },
    },
  ])
})

test('keeps the existing knowledge tool names and allowedTools compatibility', () => {
  const tools = createKnowledgeTools(() => {})
  const names = tools.map(({ name }) => name)
  const expectedNames = Object.values(KNOWLEDGE_TOOL_NAMES)

  assert.deepEqual(names, expectedNames)
  assert.equal(names.includes('list_local_knowledge_bases'), false)
  assert.equal(names.includes('search_local_knowledge'), false)
  assert.deepEqual(
    runtimeToolNames().filter((name) => expectedNames.includes(name)),
    expectedNames,
  )

  const diagnostics = diagnoseToolsForAgentContext(tools, null, {
    packageManifest: { allowedTools: expectedNames },
  })
  assert.deepEqual(diagnostics.effectiveToolNames, expectedNames)
  assert.deepEqual(diagnostics.excludedTools, [])
})

test('publishes source-aware schemas while keeping legacy knowledgeBaseId optional', () => {
  const tools = createKnowledgeTools(() => {})
  const listSchema = toolByName(tools, KNOWLEDGE_TOOL_NAMES.list).parameters
  const searchSchema = toolByName(tools, KNOWLEDGE_TOOL_NAMES.search).parameters
  const readSchema = toolByName(tools, KNOWLEDGE_TOOL_NAMES.read).parameters
  const graphSchema = toolByName(tools, KNOWLEDGE_TOOL_NAMES.graph).parameters
  const list = objectProperties(listSchema)
  const search = objectProperties(searchSchema)
  const read = objectProperties(readSchema)
  const graph = objectProperties(graphSchema)

  assert.deepEqual(list.target, KNOWLEDGE_REFERENCE_SCHEMA)
  assert.equal(list.targets.type, 'array')
  assert.deepEqual(list.targets.items, KNOWLEDGE_REFERENCE_SCHEMA)

  assert.equal(search.knowledgeBaseId.type, 'string')
  assert.equal(searchSchema.required?.includes('knowledgeBaseId') ?? false, false)
  assert.equal(search.targets.type, 'array')
  assert.deepEqual(search.targets.items, KNOWLEDGE_REFERENCE_SCHEMA)
  assert.equal(search.target.anyOf.length, 2)
  assert.equal(search.topK.type, 'integer')
  assert.equal(search.documentIds.type, 'array')

  assert.equal(read.knowledgeBaseId.type, 'string')
  assert.equal(readSchema.required?.includes('knowledgeBaseId') ?? false, false)
  assert.deepEqual(read.target, KNOWLEDGE_REFERENCE_SCHEMA)
  assert.equal(readSchema.required.includes('documentId'), true)

  assert.equal(graph.knowledgeBaseId.type, 'string')
  assert.equal(graph.targets.type, 'array')
  assert.deepEqual(graph.targets.items, KNOWLEDGE_REFERENCE_SCHEMA)
  assert.equal(graphSchema.required ?? undefined, undefined)
})

test('forwards legacy and source-aware knowledge requests without renaming fields', async () => {
  const requests = []
  const requestHost = async (type, payload) => {
    requests.push({ type, payload })
    return { payload: { result: { ok: true } } }
  }
  const tools = createKnowledgeTools(requestHost)

  await toolByName(tools, KNOWLEDGE_TOOL_NAMES.list).execute('source-aware-list', {
    targets: [{ source: 'local', id: 'local-kb' }],
  })
  await toolByName(tools, KNOWLEDGE_TOOL_NAMES.search).execute('legacy-search', {
    knowledgeBaseId: 'remote-kb',
    query: 'legacy query',
  })
  await toolByName(tools, KNOWLEDGE_TOOL_NAMES.search).execute('source-aware-search', {
    targets: [
      { source: 'local', id: 'local-kb', revision: 'generation:3' },
      { source: 'remote', connectionId: 'yuxi-main', id: 'remote-kb' },
    ],
    query: 'source-aware query',
    topK: 8,
    documentIds: ['document-1'],
  })
  await toolByName(tools, KNOWLEDGE_TOOL_NAMES.read).execute('source-aware-read', {
    target: { source: 'local', id: 'local-kb', revision: 'generation:3' },
    documentId: 'document-1',
  })

  assert.deepEqual(requests, [
    {
      type: 'tool.execute',
      payload: {
        toolCallId: 'source-aware-list',
        tool: KNOWLEDGE_TOOL_NAMES.list,
        input: { targets: [{ source: 'local', id: 'local-kb' }] },
      },
    },
    {
      type: 'tool.execute',
      payload: {
        toolCallId: 'legacy-search',
        tool: KNOWLEDGE_TOOL_NAMES.search,
        input: { knowledgeBaseId: 'remote-kb', query: 'legacy query' },
      },
    },
    {
      type: 'tool.execute',
      payload: {
        toolCallId: 'source-aware-search',
        tool: KNOWLEDGE_TOOL_NAMES.search,
        input: {
          targets: [
            { source: 'local', id: 'local-kb', revision: 'generation:3' },
            { source: 'remote', connectionId: 'yuxi-main', id: 'remote-kb' },
          ],
          query: 'source-aware query',
          topK: 8,
          documentIds: ['document-1'],
        },
      },
    },
    {
      type: 'tool.execute',
      payload: {
        toolCallId: 'source-aware-read',
        tool: KNOWLEDGE_TOOL_NAMES.read,
        input: {
          target: { source: 'local', id: 'local-kb', revision: 'generation:3' },
          documentId: 'document-1',
        },
      },
    },
  ])
})

test('forwards local graph requests and preserves the Host error code', async () => {
  const requests = []
  const requestHost = async (type, payload) => {
    requests.push({ type, payload })
    return {
      payload: {
        isError: true,
        error: '本地知识库尚未生成知识图谱。',
        errorDetails: {
          code: 'local_knowledge.graph_unavailable',
          retryable: false,
          details: { source: 'local' },
        },
      },
    }
  }
  const graph = toolByName(createKnowledgeTools(requestHost), KNOWLEDGE_TOOL_NAMES.graph)

  await assert.rejects(
    graph.execute('local-graph', {
      target: { source: 'local', id: 'local-kb', revision: 'generation:3' },
      keyword: 'Fox',
    }),
    (error) => {
      assert.equal(error.code, 'local_knowledge.graph_unavailable')
      assert.equal(error.retryable, false)
      assert.match(error.message, /local_knowledge\.graph_unavailable/)
      return true
    },
  )

  assert.deepEqual(requests, [{
    type: 'tool.execute',
    payload: {
      toolCallId: 'local-graph',
      tool: KNOWLEDGE_TOOL_NAMES.graph,
      input: {
        target: { source: 'local', id: 'local-kb', revision: 'generation:3' },
        keyword: 'Fox',
      },
    },
  }])
})
