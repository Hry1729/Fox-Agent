export const CAPABILITY_MANIFEST_VERSION = 1

export const RUNTIME_TOOL_CATALOG = Object.freeze([
  Object.freeze({ name: 'read', category: 'project-read', execution: 'runtime', approval: 'preflight' }),
  Object.freeze({ name: 'ls', category: 'project-read', execution: 'runtime', approval: 'preflight' }),
  Object.freeze({ name: 'find', category: 'project-read', execution: 'runtime', approval: 'preflight' }),
  Object.freeze({ name: 'grep', category: 'project-read', execution: 'runtime', approval: 'preflight' }),
  Object.freeze({ name: 'read_attachment', category: 'attachment', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'write_file', category: 'project-write', execution: 'host', approval: 'policy' }),
  Object.freeze({ name: 'edit_file', category: 'project-write', execution: 'host', approval: 'policy' }),
  Object.freeze({ name: 'run_command', category: 'process', execution: 'host', approval: 'always' }),
  Object.freeze({ name: 'list_knowledge_bases', category: 'knowledge', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'search_knowledge', category: 'knowledge', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'read_knowledge_document', category: 'knowledge', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'query_knowledge_graph', category: 'knowledge', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'list_mcp_tools', category: 'mcp', execution: 'host', approval: 'none' }),
  Object.freeze({ name: 'call_mcp_tool', category: 'mcp', execution: 'host', approval: 'always' }),
])

export function runtimeToolNames() {
  return RUNTIME_TOOL_CATALOG.map(({ name }) => name)
}

export function createCapabilityManifest(overrides = {}) {
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
    tools: RUNTIME_TOOL_CATALOG.map((tool) => ({ ...tool })),
    ...overrides,
  }
}

export function validateCapabilityManifest(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return 'capabilities must be an object'
  if (value.manifestVersion !== CAPABILITY_MANIFEST_VERSION) return 'unsupported capability manifest version'
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
  if (!Array.isArray(value.tools)) return 'tools must be an array'
  for (const tool of value.tools) {
    if (!tool || typeof tool !== 'object' || Array.isArray(tool)) return 'tool entries must be objects'
    if (typeof tool.name !== 'string' || !tool.name) return 'tool name is required'
    if (!['project-read', 'project-write', 'attachment', 'process', 'knowledge', 'skill', 'mcp'].includes(tool.category)) {
      return `unsupported tool category: ${tool.category}`
    }
    if (!['runtime', 'host', 'remote'].includes(tool.execution)) return `unsupported tool execution: ${tool.execution}`
    if (!['none', 'preflight', 'policy', 'always'].includes(tool.approval)) return `unsupported tool approval: ${tool.approval}`
  }
  const names = value.tools.map(({ name }) => name)
  if (new Set(names).size !== names.length) return 'tools must not contain duplicates'
  return null
}

export function assertRegisteredToolsMatchCatalog(tools) {
  const registered = tools.map(({ name }) => name)
  const expected = runtimeToolNames()
  if (registered.length !== expected.length || registered.some((name, index) => name !== expected[index])) {
    throw new Error(`Runtime tool registry does not match the capability catalog: ${registered.join(', ')}`)
  }
}
