/**
 * 将 FoxOps toolCall 映射为 AI Elements ToolHeader 所需 props。
 * state 对齐 AI SDK ToolUIPart['state']。
 */
import { getToolCallId, parseToolCallArgs } from '@/components/ToolCallingResult/toolRegistry'

/**
 * @param {object} toolCall
 * @param {{ status?: string }} [options] 外部覆盖状态：running | completed | failed
 * @returns {{ name: string, type: string, state: string, toolCallId: string, title: string, input: unknown, output: unknown, errorText?: string }}
 */
export function toElementsToolProps(toolCall, options = {}) {
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
