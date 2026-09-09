import type { ApprovalRecord } from './types'

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}
}

function text(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined
}

/** Presentation only: never changes approval policy or the tool arguments. */
export function approvalPresentation(approval: Pick<ApprovalRecord, 'request' | 'toolName' | 'requestedAction'>) {
  const request = approval.request
  const input = record(request.input)
  return {
    title: text(request.title) ?? '允许 Fox 执行此操作？',
    target: text(request.target) ?? text(input.path) ?? text(input.file_path) ?? text(request.cwd) ?? text(input.cwd) ?? approval.toolName,
    summary: text(request.summary) ?? approval.requestedAction,
    command: text(request.command) ?? text(input.command),
    diff: text(request.diff) ?? text(input.diff),
    content: text(input.content),
  }
}
