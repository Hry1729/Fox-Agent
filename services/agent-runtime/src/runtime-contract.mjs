export const CAPABILITY_MANIFEST_VERSION = 2
export const GRAPH_READONLY_TOOL_NAME = 'graph_readonly_run'

export const KNOWLEDGE_TOOL_NAMES = Object.freeze({
  list: 'list_knowledge_bases',
  search: 'search_knowledge',
  read: 'read_knowledge_document',
  graph: 'query_knowledge_graph',
})

export const RUNTIME_TOOL_CATALOG = Object.freeze([
  Object.freeze({ name: 'read', category: 'project-read', execution: 'runtime', approval: 'preflight' }),
  Object.freeze({ name: 'ls', category: 'project-read', execution: 'runtime', approval: 'preflight' }),
  Object.freeze({ name: 'find', category: 'project-read', execution: 'runtime', approval: 'preflight' }),
  Object.freeze({ name: 'grep', category: 'project-read', execution: 'runtime', approval: 'preflight' }),
  Object.freeze({ name: GRAPH_READONLY_TOOL_NAME, category: 'project-read', execution: 'runtime', approval: 'none' }),
  Object.freeze({ name: 'read_attachment', category: 'attachment', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'write_file', category: 'project-write', execution: 'host', approval: 'policy' }),
  Object.freeze({ name: 'edit_file', category: 'project-write', execution: 'host', approval: 'policy' }),
  Object.freeze({ name: 'run_command', category: 'process', execution: 'host', approval: 'always' }),
  Object.freeze({ name: 'web_search', category: 'skill', execution: 'host', approval: 'always' }),
  Object.freeze({ name: 'web_read', category: 'skill', execution: 'host', approval: 'always' }),
  Object.freeze({ name: 'http_request', category: 'skill', execution: 'host', approval: 'always' }),
  Object.freeze({ name: 'system_info', category: 'skill', execution: 'host', approval: 'always' }),
  Object.freeze({ name: 'sqlite_read', category: 'project-read', execution: 'host', approval: 'always' }),
  Object.freeze({ name: 'structured_data', category: 'skill', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'git_read', category: 'project-read', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'test_run', category: 'process', execution: 'host', approval: 'always' }),
  Object.freeze({ name: 'code_check', category: 'process', execution: 'host', approval: 'always' }),
  Object.freeze({ name: 'format_code', category: 'project-write', execution: 'host', approval: 'always' }),
  Object.freeze({ name: 'tabular_data', category: 'project-read', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'work_snapshot_get', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'graph_readonly_activate', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'graph_readonly_snapshot_get', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'graph_readonly_node_start', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'graph_readonly_node_review', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'graph_readonly_node_finish', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'graph_readonly_node_cancel', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'graph_readonly_accept', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'workflow_snapshot_get', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'workflow_start', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'workflow_stage_start', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'workflow_stage_complete', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'workflow_stage_fail', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'workflow_cancel', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'team_snapshot_get', category: 'delegation', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'team_start', category: 'delegation', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'team_member_start', category: 'delegation', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'team_collect', category: 'delegation', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'team_cancel', category: 'delegation', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'child_agent_list', category: 'delegation', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'child_run_start', category: 'delegation', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'child_run_collect', category: 'delegation', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'child_run_cancel', category: 'delegation', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'goal_propose', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'goal_complete', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'task_create_many', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'task_update', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'task_attempt_start', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'task_repair_start', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'task_repair_escalate_start', category: 'work', execution: 'host', approval: 'always' }),
  Object.freeze({ name: 'task_attempt_finish', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'task_evidence_add', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'task_evidence_validate', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'plan_revision_create', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'review_finding_add', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'review_finding_resolve', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'acceptance_submit', category: 'work', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'memory_search', category: 'memory', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'memory_propose', category: 'memory', execution: 'host', approval: 'none' }),
  Object.freeze({ name: KNOWLEDGE_TOOL_NAMES.list, category: 'knowledge', execution: 'host', approval: 'none' }),
  Object.freeze({ name: KNOWLEDGE_TOOL_NAMES.search, category: 'knowledge', execution: 'host', approval: 'none' }),
  Object.freeze({ name: KNOWLEDGE_TOOL_NAMES.read, category: 'knowledge', execution: 'host', approval: 'none' }),
  Object.freeze({ name: KNOWLEDGE_TOOL_NAMES.graph, category: 'knowledge', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'list_mcp_tools', category: 'mcp', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'call_mcp_tool', category: 'mcp', execution: 'host', approval: 'always' }),
])

export function runtimeToolNames() {
  return RUNTIME_TOOL_CATALOG.map(({ name }) => name)
}

export function createCapabilityManifest(overrides = {}) {
  const workLoop = overrides.workLoop ?? true
  const tools = overrides.tools ?? RUNTIME_TOOL_CATALOG
    .filter((tool) => workLoop || tool.category !== 'work')
    .map((tool) => ({ ...tool }))
  return {
    manifestVersion: CAPABILITY_MANIFEST_VERSION,
    streamingText: true,
    cancellation: true,
    reasoning: true,
    sessionResume: true,
    toolApproval: true,
    imageInput: false,
    steering: false,
    contextCompaction: true,
    dynamicModelSwitch: false,
    workLoop,
    tools,
    ...overrides,
  }
}

export function validateCapabilityManifest(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return 'capabilities must be an object'
  if (![1, CAPABILITY_MANIFEST_VERSION].includes(value.manifestVersion)) return 'unsupported capability manifest version'
  for (const field of [
    'streamingText',
    'cancellation',
    'reasoning',
    'sessionResume',
    'toolApproval',
    'imageInput',
    'steering',
    'contextCompaction',
    'dynamicModelSwitch',
  ]) {
    if (typeof value[field] !== 'boolean') return `${field} must be a boolean`
  }
  const workLoop = value.manifestVersion >= 2 ? value.workLoop : false
  if (value.manifestVersion >= 2 && typeof value.workLoop !== 'boolean') return 'workLoop must be a boolean'
  if (!Array.isArray(value.tools)) return 'tools must be an array'
  for (const tool of value.tools) {
    if (!tool || typeof tool !== 'object' || Array.isArray(tool)) return 'tool entries must be objects'
    if (typeof tool.name !== 'string' || !tool.name) return 'tool name is required'
    if (!['project-read', 'project-write', 'attachment', 'process', 'knowledge', 'skill', 'mcp', 'work', 'memory', 'delegation'].includes(tool.category)) {
      return `unsupported tool category: ${tool.category}`
    }
    if (!['runtime', 'host', 'remote'].includes(tool.execution)) return `unsupported tool execution: ${tool.execution}`
    if (!['none', 'preflight', 'policy', 'always'].includes(tool.approval)) return `unsupported tool approval: ${tool.approval}`
    if (tool.category === 'work' && !workLoop) return 'work tools require workLoop capability'
  }
  const names = value.tools.map(({ name }) => name)
  if (new Set(names).size !== names.length) return 'tools must not contain duplicates'
  return null
}

export function assertRegisteredToolsMatchCatalog(tools, { workLoop = true } = {}) {
  const registered = tools.map(({ name }) => name)
  const expected = RUNTIME_TOOL_CATALOG
    .filter((tool) => workLoop || tool.category !== 'work')
    .map(({ name }) => name)
  if (registered.length !== expected.length || registered.some((name, index) => name !== expected[index])) {
    throw new Error(`Runtime tool registry does not match the capability catalog: ${registered.join(', ')}`)
  }
}
