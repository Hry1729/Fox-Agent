import type { AgentRecord, DesktopErrorDetails, RunEventRecord } from '@/features/conversations/model/types'

export type ExpertBindingState = 'active' | 'replaced' | 'removed'

export interface ExpertToolAvailability {
  source: 'snapshot' | 'declaration'
  availableCount: number | null
  declaredCount: number
  label: string
  emptyIntersection: boolean
  textOnlyAvailable: true
}

export interface ExpertErrorPresentation {
  kind: 'offline' | 'locked' | 'missing' | 'invalid' | 'snapshot'
  title: string
  description: string
  retryable: boolean
  retryLabel?: string
}

export interface ExpertBindingTimelineSource {
  id: string
  expertId: string
  state: ExpertBindingState
  activatedAt: string | number
}

export type ExpertTimelineEntry<Message, Binding> =
  | { kind: 'message'; message: Message; timestamp: number }
  | { kind: 'expert'; binding: Binding; timestamp: number }

function timestamp(value: string | number) {
  const parsed = typeof value === 'number' ? value : Date.parse(value)
  return Number.isFinite(parsed) ? parsed : 0
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null
}

function stringList(value: unknown) {
  if (!Array.isArray(value)) return []
  return [...new Set(value.flatMap((item) => {
    if (typeof item === 'string' && item.trim()) return [item.trim()]
    const itemRecord = record(item)
    const name = itemRecord?.name ?? itemRecord?.id ?? itemRecord?.toolName
    return typeof name === 'string' && name.trim() ? [name.trim()] : []
  }))]
}

function finiteCount(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0
    ? Math.floor(value)
    : null
}

function firstArray(...values: unknown[]) {
  return values.find((value): value is unknown[] => Array.isArray(value)) ?? null
}

function latestExpertSnapshot(events: RunEventRecord[], expertId: string) {
  return [...events].reverse().find((item) => {
    if (item.eventType !== 'run.request_snapshot') return false
    const snapshot = record(item.event)
    const binding = record(snapshot?.expertBinding)
    const expertPackage = record(snapshot?.expertPackage)
    const snapshotExpertId = binding?.expertId ?? expertPackage?.id
    return snapshotExpertId === expertId
  })?.event
}

function snapshotToolAvailability(snapshotValue: unknown): ExpertToolAvailability | null {
  const snapshot = record(snapshotValue)
  const expertPackage = record(snapshot?.expertPackage)
  if (!snapshot || !expertPackage) return null
  const toolDiagnostics = record(snapshot.expertToolDiagnostics)
    ?? record(record(snapshot.toolDiagnostics)?.expert)
    ?? record(record(snapshot.expertDiagnostics)?.tools)
  const packageManifest = record(expertPackage.packageManifest)
  const resources = record(expertPackage.resources)
  const declaredSource = firstArray(
    snapshot.expertDeclaredToolNames,
    toolDiagnostics?.declaredToolNames,
    toolDiagnostics?.declaredTools,
    toolDiagnostics?.declared,
    packageManifest?.allowedTools,
    resources?.tools,
  )
  const effectiveSource = firstArray(
    snapshot.effectiveToolNames,
    toolDiagnostics?.effectiveToolNames,
    toolDiagnostics?.availableToolNames,
    toolDiagnostics?.authorizedToolNames,
    toolDiagnostics?.effectiveTools,
    toolDiagnostics?.available,
  )
  const declaredNames = stringList(declaredSource)
  const effectiveNames = stringList(effectiveSource)
  const runToolNames = stringList(snapshot.toolNames)
  const declaredSet = new Set(declaredNames)
  const effectiveCandidates = effectiveSource ? effectiveNames : runToolNames
  const derivedEffectiveNames = declaredSource
    ? effectiveCandidates.filter((name) => declaredSet.has(name))
    : effectiveCandidates
  const explicitDeclaredCount = finiteCount(toolDiagnostics?.declaredCount)
    ?? finiteCount(toolDiagnostics?.totalCount)
  const explicitAvailableCount = finiteCount(toolDiagnostics?.availableCount)
    ?? finiteCount(toolDiagnostics?.effectiveCount)
  const availableCount = explicitAvailableCount ?? derivedEffectiveNames.length
  const declaredCount = Math.max(
    availableCount,
    explicitDeclaredCount ?? (declaredSource ? declaredNames.length : runToolNames.length),
  )
  return {
    source: 'snapshot',
    availableCount,
    declaredCount,
    label: `上次运行 ${availableCount}/${declaredCount} 工具可用`,
    emptyIntersection: availableCount === 0,
    textOnlyAvailable: true,
  }
}

export function declaredExpertToolAvailability(expert: AgentRecord): ExpertToolAvailability {
  const declaredTools = Array.isArray(expert.packageManifest.allowedTools)
    ? stringList(expert.packageManifest.allowedTools)
    : expert.resources.tools.map((tool) => tool.id || tool.name).filter(Boolean)
  const summaries = [
    declaredTools.length ? `${declaredTools.length} 个工具` : '',
    expert.resources.skills.length ? `${expert.resources.skills.length} 个 Skills` : '',
    expert.resources.knowledges.length ? `${expert.resources.knowledges.length} 个知识资源` : '',
    expert.resources.mcps.length ? `${expert.resources.mcps.length} 个 MCP` : '',
  ].filter(Boolean)
  return {
    source: 'declaration',
    availableCount: null,
    declaredCount: declaredTools.length,
    label: summaries.length ? `声明能力：${summaries.join(' · ')}` : '未声明专用工具能力',
    emptyIntersection: false,
    textOnlyAvailable: true,
  }
}

export function latestExpertToolAvailability(
  events: RunEventRecord[],
  expert: AgentRecord,
): ExpertToolAvailability {
  return snapshotToolAvailability(latestExpertSnapshot(events, expert.id))
    ?? declaredExpertToolAvailability(expert)
}

function errorDetails(value: DesktopErrorDetails | Error | string | null | undefined) {
  if (typeof value === 'string') return { code: '', message: value, retryable: false }
  if (value instanceof Error) {
    const coded = value as Error & { code?: unknown; retryable?: unknown }
    return {
      code: typeof coded.code === 'string' ? coded.code : '',
      message: value.message,
      retryable: coded.retryable === true,
    }
  }
  return value ?? { code: '', message: '', retryable: false }
}

export function expertErrorPresentation(
  value: DesktopErrorDetails | Error | string | null | undefined,
): ExpertErrorPresentation | null {
  const details = errorDetails(value)
  const code = details.code.toLowerCase()
  const message = details.message.toLowerCase()
  const expertScoped = code.includes('expert') || /expert|专家/.test(message)
  if (!expertScoped) return null

  if (
    code === 'conversation.expert_snapshot_invalid'
    || /expert\.(snapshot|package_hash|package).*(corrupt|invalid|mismatch|damaged)/.test(code)
    || /snapshot.*(corrupt|invalid)|package hash.*mismatch|快照.*(损坏|无效)|校验.*失败/.test(message)
  ) {
    return {
      kind: 'snapshot',
      title: '专家快照已损坏',
      description: '为避免加载错误能力，请新建对话并重新选择该专家。',
      retryable: false,
    }
  }
  if (
    code === 'conversation.expert_locked'
    || /expert.*(binding_)?locked|conversation\.expert_(bind|remove)_locked/.test(code)
    || /cannot change.*(?:messages|run)|绑定.*锁定|已有消息.*专家|运行中.*专家/.test(message)
  ) {
    return {
      kind: 'locked',
      title: '当前对话的专家已锁定',
      description: '请新建对话后再更换或移除专家，当前对话内容不会受影响。',
      retryable: false,
    }
  }
  if (
    code === 'conversation.expert_remote_unavailable'
    || /expert.*(offline|unavailable)|expert_service_unavailable/.test(code)
    || /expert.*(?:offline|unavailable)|专家.*(?:离线|不可用)|服务离线/.test(message)
  ) {
    return {
      kind: 'offline',
      title: '专家当前离线',
      description: '请检查专家服务连接，或更换其他专家后重试。',
      retryable: true,
      retryLabel: '重试连接',
    }
  }
  if (
    /expert.*(?:not_found|missing|deleted)/.test(code)
    || /expert (?:was|is) not found|conversation expert was not found|未找到专家|专家.*(?:不存在|已删除)/.test(message)
  ) {
    return {
      kind: 'missing',
      title: '专家不存在或已被删除',
      description: '请返回专家中心，重新选择一个可用专家。',
      retryable: false,
    }
  }
  if (
    code === 'conversation.expert_invalid_role'
    || /expert.*(?:invalid|required)|conversation\.expert_required/.test(code)
    || /selected agent is not an expert|专家.*(?:无效|配置异常|类型不正确)|不是专家/.test(message)
  ) {
    return {
      kind: 'invalid',
      title: '专家配置无效',
      description: '请在专家中心检查配置，或更换其他专家。',
      retryable: false,
    }
  }
  return null
}

export function expertErrorMessage(
  value: DesktopErrorDetails | Error | string | null | undefined,
  fallback: string,
) {
  const presentation = expertErrorPresentation(value)
  if (presentation) return `${presentation.title}：${presentation.description}`
  const details = errorDetails(value)
  return details.message.trim() || fallback
}

export function mergeExpertBindingsIntoTimeline<
  Message extends { createdAt: number },
  Binding extends ExpertBindingTimelineSource,
>(messages: Message[], bindings: Binding[]): Array<ExpertTimelineEntry<Message, Binding>> {
  return [
    ...messages.map((message) => ({ kind: 'message' as const, message, timestamp: timestamp(message.createdAt) })),
    ...bindings.map((binding) => ({ kind: 'expert' as const, binding, timestamp: timestamp(binding.activatedAt) })),
  ].sort((left, right) => left.timestamp - right.timestamp || (left.kind === right.kind ? 0 : left.kind === 'expert' ? -1 : 1))
}

export function expertInteractionLocked({
  running,
  approvalPending,
  questionPending,
}: {
  running: boolean
  approvalPending: boolean
  questionPending: boolean
}) {
  return running || approvalPending || questionPending
}
