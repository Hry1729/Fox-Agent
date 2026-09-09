export type ReconciliationDecision = 'executed' | 'not_executed' | 'unresolved'
export interface ReconciliationItem {
  effectKey: string
  tool: string
  input: Record<string, unknown>
  evidence: Record<string, unknown> | null
  decision: ReconciliationDecision | null
  note: string | null
}
export interface ReconciliationView {
  runId: string
  revision: number
  items: ReconciliationItem[]
  recoveryRunId: string | null
  canResume: boolean
}
export interface ReconciliationRequest {
  conversationId: string
  runId: string
  expectedRevision?: number
  effectKey?: string
  decision?: ReconciliationDecision
  note?: string
  queryTool?: string
  arguments?: Record<string, unknown>
  recoveryMode?: 'read_only' | 'reapprove'
}
export interface ReconciliationOptions {
  kind: 'file' | 'connector' | 'manual'
  message: string
  tools: { name: string; description: string; inputSchema: Record<string, unknown> }[]
}
export function reconciliationReady(view: ReconciliationView | null): boolean {
  return Boolean(view?.canResume && !view.recoveryRunId && view.items.length && view.items.every(
    item => (item.decision === 'executed' || item.decision === 'not_executed') && item.note?.trim(),
  ))
}
export function parseQueryArguments(text: string): Record<string, unknown> {
  if (new TextEncoder().encode(text).length > 16384) throw new Error('查询条件过长，请减少内容。')
  const value: unknown = JSON.parse(text)
  if (!value || Array.isArray(value) || typeof value !== 'object') throw new Error('查询条件需要是一个对象。')
  return value as Record<string, unknown>
}
