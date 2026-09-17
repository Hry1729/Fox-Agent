import { boundToolText, modelToolResultContent, toolResultRef, RECEIPT_MARKER, RESULT_REF_TOOL } from './tool-view.mjs'

export const FOX_TOOL_DEFINITION_VERSION = 1

const TOOL_NAME_PATTERN = /^[a-z][a-z0-9_]{0,63}$/
const EXECUTION_MODES = new Set(['runtime', 'host'])
const TOOL_EXECUTION_MODES = new Set(['sequential', 'parallel'])
// Results at or below this many JS characters pass through unchanged (their
// JSON stays intact). Above it the model view is produced by the SAME
// projection the kernel path uses (tool-view.mjs): structure-aware, UTF-8
// code-point safe, with `next` and stable references preserved — never a
// mid-JSON character cut, and never a retrieval promise without the Host's
// trusted storage fact.
const MAX_MODEL_VISIBLE_RESULT_CHARS = 120_000
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

function isPiContentBlock(block) {
  if (!block || typeof block !== 'object' || Array.isArray(block)) return false
  if (block.type === 'text') return typeof block.text === 'string'
  if (block.type === 'image') return typeof block.data === 'string'
  return false
}

function isPiToolResult(result) {
  return Boolean(
    result
    && typeof result === 'object'
    && !Array.isArray(result)
    && Array.isArray(result.content)
    && result.content.every(isPiContentBlock),
  )
}

function stringifyToolResult(result) {
  if (typeof result === 'string') return result
  if (result === undefined) return 'Tool completed successfully without a return value.'

  const seen = new WeakSet()
  try {
    const serialized = JSON.stringify(result, (_key, value) => {
      if (typeof value === 'bigint') return value.toString()
      if (value && typeof value === 'object') {
        if (seen.has(value)) return '[Circular]'
        seen.add(value)
      }
      return value
    })
    return serialized ?? String(result)
  } catch (error) {
    return `[Tool result could not be serialized: ${error instanceof Error ? error.message : String(error)}]`
  }
}

function jsonSafeToolDetails(result, serialized) {
  if (result === undefined) return { kind: 'undefined' }
  if (typeof result === 'string') return result
  try {
    return JSON.parse(serialized)
  } catch {
    return String(result)
  }
}

/**
 * Normalize one settled Fox tool result into the Pi model view.
 *
 * Two invariants (ABC review R4/R5):
 *   1. A structured payload is never character-cut. If the unified projection
 *      declines to shape it, the payload is published **intact** — an invalid
 *      JSON fragment would hide records with no way back, and claiming Host
 *      stored them without a trusted fact would be a false promise.
 *   2. Retrieval is promised only from the Host's trusted storage fact
 *      (`viewContext.storage`), never from the result's size, never from a
 *      reference alone, and never from metadata the tool result declares
 *      about itself.
 */
export function normalizeFoxToolResultForPi(toolName, result, viewContext = {}) {
  if (isPiToolResult(result)) {
    // `read_tool_result` returns its range-cursor in `details`, which the
    // provider projection drops. Surface the whitelisted navigation facts as a
    // trailing text block so a multi-page Legacy read stays reachable.
    if (result.isError !== true && toolName === RESULT_REF_TOOL) {
      return { ...result, content: modelToolResultContent(toolName, result), details: result.details ?? {} }
    }
    return result.details === undefined ? { ...result, details: {} } : result
  }

  const serialized = stringifyToolResult(result)
  if (serialized.length <= MAX_MODEL_VISIBLE_RESULT_CHARS) {
    return {
      content: [{ type: 'text', text: serialized }],
      details: { structuredResult: jsonSafeToolDetails(result, serialized), outputTruncated: false },
    }
  }
  // An execution receipt is evidence and is never reshaped, however large.
  if (serialized.includes(RECEIPT_MARKER)) {
    return {
      content: [{ type: 'text', text: serialized }],
      details: { outputTruncated: false },
    }
  }
  // Oversized: project with the same rules as the kernel path. `boundToolText`
  // keeps JSON valid, preserves `next`, and returns the original text when no
  // honest bounded view exists — in which case nothing is omitted at all.
  // The storage fact is the Host's; the adapter never trusts a `details.storage`
  // that the tool result declared about itself.
  const reference = toolResultRef(viewContext.runId, viewContext.toolCallId)
  const projected = boundToolText(toolName, {
    text: serialized,
    resultRef: reference,
    storage: viewContext.storage ?? null,
  })
  if (projected !== null) {
    return {
      content: [{ type: 'text', text: projected }],
      details: { outputTruncated: projected !== serialized, unifiedView: true },
    }
  }
  // The unified projection declined (not re-readable reference material, or no
  // honest bounded view): publish the payload intact. There is no structure
  // damage and no retrieval promise to retract.
  return {
    content: [{ type: 'text', text: serialized }],
    details: {
      structuredResult: jsonSafeToolDetails(result, serialized),
      outputTruncated: false,
      projectionDeclined: true,
    },
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

export function adaptFoxToolToPi(tool, viewContext = {}) {
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
    // viewContext.runId lets the unified model-view projection mint the stable
    // `fox-result://` reference for oversized results. Retrieval is promised
    // only when `viewContext.storageFor` (the Host's trusted storage fact for
    // this settled call) says the complete result was stored; without that fact
    // the view keeps every byte and promises nothing.
    execute: async (toolCallId, ...rest) => {
      const result = await tool.execute(toolCallId, ...rest)
      const storage = typeof viewContext.storageFor === 'function'
        ? viewContext.storageFor(toolCallId, result)
        : (viewContext.storage ?? null)
      return normalizeFoxToolResultForPi(tool.name, result, {
        runId: viewContext.runId,
        toolCallId,
        storage,
      })
    },
    ...optionalFields,
  }
}

export function adaptFoxToolsToPi(tools, viewContext = {}) {
  if (!Array.isArray(tools)) {
    throw adapterError('tool_adapter.invalid_collection', 'Fox tools must be an array.')
  }
  ensureUniqueNames(tools)
  return tools.map((tool) => adaptFoxToolToPi(tool, viewContext))
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
