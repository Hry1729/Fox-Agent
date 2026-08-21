import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = dirname(fileURLToPath(import.meta.url))
const root = join(__dirname, '..')

function getToolCallId(toolCall) {
  return toolCall?.name || toolCall?.function?.name || ''
}

function parseToolCallArgs(toolCall) {
  const args = toolCall?.args ?? toolCall?.function?.arguments
  if (!args) return {}
  if (typeof args === 'object') return args
  try {
    return JSON.parse(args)
  } catch {
    return {}
  }
}

function toElementsToolProps(toolCall, options = {}) {
  const toolCallId = getToolCallId(toolCall) || 'tool'
  const type = `tool-${toolCallId}`
  const input = parseToolCallArgs(toolCall) || {}
  const output = toolCall?.tool_call_result?.content
  const errorText =
    typeof toolCall?.tool_call_result?.error === 'string'
      ? toolCall.tool_call_result.error
      : typeof toolCall?.error === 'string'
        ? toolCall.error
        : undefined

  const external = options.status || ''
  let state = 'input-streaming'
  if (external === 'failed' || toolCall?.status === 'error' || errorText) {
    state = 'output-error'
  } else if (
    external === 'completed' ||
    toolCall?.status === 'success' ||
    toolCall?.tool_call_result != null
  ) {
    state = 'output-available'
  } else if (toolCall?.args || toolCall?.function?.arguments) {
    state = 'input-available'
  } else if (external === 'running') {
    state = 'input-streaming'
  }

  return {
    name: toolCallId,
    type,
    state,
    toolCallId,
    title: toolCallId,
    input,
    output,
    errorText
  }
}

// 1) 运行中 → input-streaming / input-available
{
  const streaming = toElementsToolProps({ name: 'bash' })
  assert.equal(streaming.name, 'bash')
  assert.equal(streaming.type, 'tool-bash')
  assert.equal(streaming.state, 'input-streaming')

  const withArgs = toElementsToolProps({
    name: 'bash',
    args: { command: 'ls' }
  })
  assert.equal(withArgs.state, 'input-available')
  assert.deepEqual(withArgs.input, { command: 'ls' })
}

// 2) 成功 → output-available
{
  const ok = toElementsToolProps({
    name: 'calculator',
    status: 'success',
    args: '{"expression":"1+1"}',
    tool_call_result: { content: '2' }
  })
  assert.equal(ok.state, 'output-available')
  assert.equal(ok.output, '2')
  assert.deepEqual(ok.input, { expression: '1+1' })
}

// 3) 失败 → output-error
{
  const err = toElementsToolProps({
    name: 'read_file',
    status: 'error',
    error: 'file not found'
  })
  assert.equal(err.state, 'output-error')
  assert.equal(err.errorText, 'file not found')
}

// 源码接线
{
  const adapter = readFileSync(join(root, 'src/utils/chat/toolElementsAdapter.js'), 'utf8')
  assert.match(adapter, /export function toElementsToolProps/)
  const base = readFileSync(join(root, 'src/components/ToolCallingResult/BaseToolCall.vue'), 'utf8')
  assert.match(base, /from ['"]@\/components\/ai-elements\/tool['"]/)
  assert.match(base, /ToolHeader/)
  const el = readFileSync(
    join(root, 'src/components/ToolCallingResult/ElementsToolCall.vue'),
    'utf8'
  )
  assert.match(el, /TOOL_RENDERERS/)
  const renderer = readFileSync(
    join(root, 'src/components/ToolCallingResult/ToolCallRenderer.vue'),
    'utf8'
  )
  assert.match(renderer, /ElementsToolCall/)
}

console.log('chat-tool-elements-adapter.test.mjs: ok')
