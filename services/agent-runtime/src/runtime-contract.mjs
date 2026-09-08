import { CAPABILITY_MANIFEST_VERSION, RUNTIME_TOOL_CATALOG, validateWireValue } from '../../../packages/fox-engine-protocol/index.mjs'
export { CAPABILITY_MANIFEST_VERSION, RUNTIME_TOOL_CATALOG }
export const GRAPH_READONLY_TOOL_NAME = 'graph_readonly_run'

export const KNOWLEDGE_TOOL_NAMES = Object.freeze({
  list: 'list_knowledge_bases',
  search: 'search_knowledge',
  read: 'read_knowledge_document',
  graph: 'query_knowledge_graph',
})

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
    const canonical = RUNTIME_TOOL_CATALOG.find(entry => entry.name === tool.name)
    if (!canonical) return `tool is not in the canonical catalog: ${tool.name}`
    if (['category', 'execution', 'approval'].some(field => tool[field] !== canonical[field])) return `tool contract mismatch: ${tool.name}`
  }
  const names = value.tools.map(({ name }) => name)
  if (new Set(names).size !== names.length) return 'tools must not contain duplicates'
  return validateWireValue('RuntimeCapabilityManifest', value)[0] ?? null
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
