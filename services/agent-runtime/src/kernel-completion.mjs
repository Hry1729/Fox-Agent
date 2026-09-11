// A model-owned final-answer signal. This is not a task-specific acceptance
// judge and never grants tools, creates artifacts, or starts another request.
export const KERNEL_FINAL_MARKER = '<fox-final/>'
export const KERNEL_COMPLETION_CONTRACT = `Fox final-answer protocol v1:
Decide yourself whether the current user request is ready for a final answer, using the actual conversation and tool results.
For an action request, continue with available tool calls until you have done the authorized work or reached a concrete blocker. A plan, acknowledgement, or promise to start is not a final answer. Do not end an action request with only "Let me analyze" or an equivalent preparation sentence.
Before finishing, check the user's requested deliverables against the actual results. Do not invent files, calculations, successful operations, or verification. If files or tools are unavailable, explain the specific blocker or ask the necessary question. Ordinary questions, refusals, and requests for missing information can be final answers without tool calls.
When you decide to give a final answer, write the useful answer in the user's language, then append ${KERNEL_FINAL_MARKER} alone on the last line. The adapter removes this internal marker before displaying or storing the answer. Do not append it to a progress update or a tool-call message. A normal provider stop without this explicit signal is an incomplete response, not a declaration of completion.`

export class KernelIncompleteResponseError extends Error {
  constructor() { super('Kernel model stopped without a completed public response'); this.name = 'KernelIncompleteResponseError' }
}

export function completionRequired(systemPrompt) {
  // Opt in only through the frozen prompt. Old, already-frozen runs and
  // dedicated compaction requests retain their original output contract.
  return typeof systemPrompt === 'string' && systemPrompt.includes(KERNEL_COMPLETION_CONTRACT)
}

export function finalizeKernelAnswer(message, required = false) {
  if (message.stopReason !== 'stop' || !Array.isArray(message.content)
      || message.content.some(block => !(block?.type === 'text' && typeof block.text === 'string'
        || block?.type === 'thinking' && typeof block.thinking === 'string'))) return message
  const text = message.content.filter(block => block.type === 'text').map(block => block.text).join('')
  const match = required ? text.match(/(?:^|\r?\n)<fox-final\/>\s*$/) : null
  if (!text.trim() || required && (!match || !text.slice(0, match.index).trim())) throw new KernelIncompleteResponseError()
  if (!required) return message
  let remove = text.length - match.index
  const content = structuredClone(message.content)
  for (let index = content.length - 1; index >= 0 && remove; index--) {
    const block = content[index]
    if (block.type !== 'text') continue
    const count = Math.min(remove, block.text.length)
    block.text = block.text.slice(0, block.text.length - count)
    remove -= count
  }
  return { ...message, content: content.filter(block => block.type !== 'text' || block.text.length) }
}

export function completionPreview(text, required = false) {
  if (!required) return text
  const complete = text.match(/(?:^|\r?\n)<fox-final\/>\s*$/)
  if (complete) return text.slice(0, complete.index)
  const newline = text.lastIndexOf('\n')
  const tail = text.slice(newline + 1)
  if ((tail && KERNEL_FINAL_MARKER.startsWith(tail)) || tail.trimEnd() === KERNEL_FINAL_MARKER) {
    return text.slice(0, Math.max(0, newline)).replace(/\r$/, '')
  }
  return text
}
