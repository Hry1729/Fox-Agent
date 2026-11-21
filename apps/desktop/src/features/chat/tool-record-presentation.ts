import type { ToolCallRecord } from '@/features/conversations/model/types'
import { compactValue, toolIcon, toolRowSlots, type RuntimeToolStep } from './timeline/fox-runtime-process-model'

export function toolRecordPresentation(tool: Pick<ToolCallRecord, 'toolName' | 'input' | 'status'>) {
  const step: RuntimeToolStep = { id: '', seq: 0, lastSeq: 0, name: tool.toolName, input: tool.input, isError: false, completed: !['running', 'pending'].includes(tool.status), awaitingUser: false }
  return { ...toolRowSlots(step), Icon: toolIcon(step), target: toolTarget(tool) }
}

export function toolTarget(tool: Pick<ToolCallRecord, 'input' | 'toolName'>) {
  const input = tool.input && typeof tool.input === 'object' && !Array.isArray(tool.input) ? tool.input as Record<string, unknown> : {}
  return compactValue(input.path ?? input.filePath ?? input.file ?? input.target ?? input.command ?? input.cmd ?? input.url ?? input.query ?? input.pattern ?? input.directory, '')
}
