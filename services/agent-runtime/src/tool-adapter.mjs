export const FOX_TOOL_DEFINITION_VERSION = 1

const TOOL_NAME_PATTERN = /^[a-z][a-z0-9_]{0,63}$/
const EXECUTION_MODES = new Set(['runtime', 'host'])
const TOOL_EXECUTION_MODES = new Set(['sequential', 'parallel'])
const PI_COMPATIBLE_OPTIONAL_FIELDS = [
  'promptSnippet',
  'promptGuidelines',
  'prepareArguments',
  'executionMode',
]

function adapterError(code, message) {
  const error = new Error(message)
  error.code = code
  return error
}

function validateToolShape(tool) {
  if (!tool || typeof tool !== 'object') {
    throw adapterError('tool_adapter.invalid_tool', 'Tool definition must be an object.')
  }
  if (typeof tool.name !== 'string' || !TOOL_NAME_PATTERN.test(tool.name)) {
    throw adapterError('tool_adapter.invalid_name', `Invalid tool name: ${String(tool.name || '')}`)
  }
  if (typeof tool.label !== 'string' || !tool.label.trim()) {
    throw adapterError('tool_adapter.invalid_label', `Tool ${tool.name} must provide a label.`)
  }
  if (typeof tool.description !== 'string' || !tool.description.trim()) {
    throw adapterError('tool_adapter.invalid_description', `Tool ${tool.name} must provide a description.`)
  }
  if (!tool.parameters || typeof tool.parameters !== 'object' || Array.isArray(tool.parameters)) {
    throw adapterError('tool_adapter.invalid_parameters', `Tool ${tool.name} must provide an object parameters schema.`)
  }
  if (typeof tool.execute !== 'function') {
    throw adapterError('tool_adapter.invalid_executor', `Tool ${tool.name} must provide an execute function.`)
  }
  if (tool.promptSnippet !== undefined && typeof tool.promptSnippet !== 'string') {
    throw adapterError('tool_adapter.invalid_prompt_snippet', `Tool ${tool.name} has an invalid prompt snippet.`)
  }
  if (tool.promptGuidelines !== undefined && (
    !Array.isArray(tool.promptGuidelines)
    || tool.promptGuidelines.some((guideline) => typeof guideline !== 'string')
  )) {
    throw adapterError('tool_adapter.invalid_prompt_guidelines', `Tool ${tool.name} has invalid prompt guidelines.`)
  }
  if (tool.prepareArguments !== undefined && typeof tool.prepareArguments !== 'function') {
    throw adapterError('tool_adapter.invalid_argument_preparer', `Tool ${tool.name} has an invalid argument preparer.`)
  }
  if (tool.executionMode !== undefined && !TOOL_EXECUTION_MODES.has(tool.executionMode)) {
    throw adapterError('tool_adapter.invalid_tool_execution_mode', `Tool ${tool.name} has an invalid execution mode.`)
  }
}

function normalizeMetadata({ source = 'fox-core', execution = 'runtime', trusted = true } = {}) {
  const normalizedSource = String(source).trim()
  if (!normalizedSource) {
    throw adapterError('tool_adapter.invalid_source', 'Tool source must not be empty.')
  }
  if (!EXECUTION_MODES.has(execution)) {
    throw adapterError('tool_adapter.invalid_execution', `Unsupported tool execution mode: ${execution}`)
  }
  return Object.freeze({
    schemaVersion: FOX_TOOL_DEFINITION_VERSION,
    source: normalizedSource,
    execution,
    trusted: Boolean(trusted),
  })
}

function ensureUniqueNames(tools) {
  const names = new Set()
  for (const tool of tools) {
    if (names.has(tool.name)) {
      throw adapterError('tool_adapter.duplicate_name', `Duplicate tool name: ${tool.name}`)
    }
    names.add(tool.name)
  }
}

export function defineFoxTool(definition, metadata) {
  validateToolShape(definition)
  const optionalFields = Object.fromEntries(
    PI_COMPATIBLE_OPTIONAL_FIELDS
      .filter((field) => definition[field] !== undefined)
      .map((field) => [field, definition[field]]),
  )
  return Object.freeze({
    name: definition.name,
    label: definition.label,
    description: definition.description,
    parameters: definition.parameters,
    execute: definition.execute,
    ...optionalFields,
    fox: normalizeMetadata(metadata),
  })
}

export function defineFoxTools(definitions, metadata) {
  if (!Array.isArray(definitions)) {
    throw adapterError('tool_adapter.invalid_collection', 'Tool definitions must be an array.')
  }
  const tools = definitions.map((definition) => defineFoxTool(definition, metadata))
  ensureUniqueNames(tools)
  return tools
}

export function adaptFoxToolToPi(tool) {
  validateToolShape(tool)
  if (tool.fox?.schemaVersion !== FOX_TOOL_DEFINITION_VERSION) {
    throw adapterError('tool_adapter.unsupported_schema', `Tool ${tool.name} does not use Fox tool schema v${FOX_TOOL_DEFINITION_VERSION}.`)
  }
  const optionalFields = Object.fromEntries(
    PI_COMPATIBLE_OPTIONAL_FIELDS
      .filter((field) => tool[field] !== undefined)
      .map((field) => [field, tool[field]]),
  )
  return {
    name: tool.name,
    label: tool.label,
    description: tool.description,
    parameters: tool.parameters,
    execute: tool.execute,
    ...optionalFields,
  }
}

export function adaptFoxToolsToPi(tools) {
  if (!Array.isArray(tools)) {
    throw adapterError('tool_adapter.invalid_collection', 'Fox tools must be an array.')
  }
  ensureUniqueNames(tools)
  return tools.map(adaptFoxToolToPi)
}

export function adaptPiToolToFox(piTool, {
  source = 'pi-extension',
  execution = 'host',
  trusted = false,
  hostExecutor,
} = {}) {
  validateToolShape(piTool)
  let execute
  if (execution === 'runtime') {
    if (!trusted) {
      throw adapterError(
        'tool_adapter.untrusted_runtime_execution',
        `Pi tool ${piTool.name} cannot execute inside the Runtime until its extension is trusted.`,
      )
    }
    execute = piTool.execute
  } else if (execution === 'host') {
    if (typeof hostExecutor !== 'function') {
      throw adapterError(
        'tool_adapter.host_executor_required',
        `Pi tool ${piTool.name} requires a Fox Host executor.`,
      )
    }
    execute = (toolCallId, input, signal) => hostExecutor({
      source: String(source).trim(),
      tool: piTool.name,
      toolCallId,
      input,
      signal,
    })
  } else {
    throw adapterError('tool_adapter.invalid_execution', `Unsupported tool execution mode: ${execution}`)
  }
  return defineFoxTool({ ...piTool, execute }, { source, execution, trusted })
}

export function collectTrustedPiExtensionTools(extensionFactory, options = {}) {
  if (typeof extensionFactory !== 'function') {
    throw adapterError('tool_adapter.invalid_extension', 'Pi extension must export a factory function.')
  }
  if (options.trusted !== true) {
    throw adapterError(
      'tool_adapter.untrusted_extension',
      'Pi extension code cannot be evaluated until the package is trusted.',
    )
  }
  const registered = []
  const api = new Proxy(Object.freeze({
    registerTool(tool) {
      registered.push(tool)
    },
  }), {
    get(target, property, receiver) {
      if (Reflect.has(target, property)) return Reflect.get(target, property, receiver)
      throw adapterError(
        'tool_adapter.unsupported_extension_api',
        `Pi extension API ${String(property)} is not available through the Fox tool adapter.`,
      )
    },
  })
  const result = extensionFactory(api)
  if (result && typeof result.then === 'function') {
    throw adapterError(
      'tool_adapter.async_extension_unsupported',
      'Pi tool extensions must register tools synchronously.',
    )
  }
  const tools = registered.map((tool) => adaptPiToolToFox(tool, options))
  ensureUniqueNames(tools)
  return tools
}
