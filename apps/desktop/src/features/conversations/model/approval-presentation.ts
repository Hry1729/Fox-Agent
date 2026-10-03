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
  const replacement = wholeFileReplacementBinding(request)
  return {
    title: text(request.title) ?? (replacement ? replacement.baselineVersion === 'missing' ? '允许 Fox 写入新文件？' : '允许 Fox 替换整个文件？' : '允许 Fox 执行此操作？'),
    target: text(request.target) ?? text(input.path) ?? text(input.file_path) ?? text(request.cwd) ?? text(input.cwd) ?? approval.toolName,
    summary: text(request.summary) ?? (replacement ? '请核对目标文件和拟写入内容。' : approval.requestedAction),
    command: text(request.command) ?? text(input.command),
    diff: text(request.diff) ?? text(input.diff),
    content: text(input.content),
    // The purpose-specific whole-file replacement binding, when this approval is
    // one. It is immutable: the Host bound it when the request was created and
    // the claim requires exactly these four facts.
    wholeFileReplacement: replacement,
    officeReuse: officeReuseBinding(request),
  }
}

export function officeReuseBinding(request: unknown): { target: string; selectors: string[] } | null {
  const candidate = record(record(request).officeReuse)
  const target = text(candidate.target)
  const selectors = candidate.selectors
  if (!target || !Array.isArray(selectors) || selectors.length === 0 || selectors.length > 16
    || !selectors.every((selector) => typeof selector === 'string' && selector.length <= 320)) return null
  return { target, selectors }
}

/// The immutable whole-file replacement binding carried by an approval request.
export type WholeFileReplacementBinding = {
  purpose: string
  requestDigest: string
  targetIdentity: string
  baselineVersion: string
  candidateDigest: string
}

export function wholeFileReplacementBinding(request: unknown): WholeFileReplacementBinding | null {
  const source = request !== null && typeof request === 'object' && !Array.isArray(request)
    ? (request as Record<string, unknown>).wholeFileReplacement
    : null
  if (source === null || typeof source !== 'object' || Array.isArray(source)) return null
  const value = source as Record<string, unknown>
  const str = (key: string) => (typeof value[key] === 'string' && value[key].trim() ? value[key] as string : null)
  const requestDigest = str('requestDigest')
  const targetIdentity = str('targetIdentity')
  const baselineVersion = str('baselineVersion')
  const candidateDigest = str('candidateDigest')
  if (!requestDigest || !targetIdentity || !baselineVersion || !candidateDigest) return null
  return {
    purpose: str('purpose') ?? '整文件替换',
    requestDigest,
    targetIdentity,
    baselineVersion,
    candidateDigest,
  }
}
