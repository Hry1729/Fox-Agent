import test from 'node:test'
import assert from 'node:assert/strict'
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

function toolByName(tools, name) {
  const tool = tools.find((entry) => entry.name === name)
  assert.ok(tool, `missing tool ${name}`)
  return tool
}

function objectProperties(schema) {
  assert.equal(schema.type, 'object')
  return schema.properties
}

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
  assert.equal(start.objective.maxLength, 8000)
  assert.equal(start.context.maxLength, 12000)
  assert.equal(start.budget.properties.maxDurationMs.maximum, 900000)
  assert.equal(start.budget.properties.maxTotalTokens.maximum, 200000)
  assert.equal(start.budget.properties.maxOutputTokens.maximum, 32768)
  assert.equal(start.budget.properties.maxToolCalls.maximum, 100)

  const collect = objectProperties(toolByName(tools, 'child_run_collect').parameters)
  assert.equal(collect.childRunIds.minItems, 1)
  assert.equal(collect.childRunIds.maxItems, 8)
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
      objective: 'Review the cancellation path',
      context: 'Inspect only the supplied run IDs.',
      agentId: 'fox-general',
      budget: { maxDurationMs: 30000, maxTotalTokens: 4000, maxOutputTokens: 1000, maxToolCalls: 4 },
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
