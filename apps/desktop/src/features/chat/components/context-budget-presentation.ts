// Presentation mapper for the Host context-budget report (CONTRACTS §5).
// Pure functions only: the Host DTO comes in, a view model goes out. The
// transport (query command / snapshot field) and the mount point are owned
// by window A; this module never fetches by itself.

export interface ContextBudgetDto {
  modelWindowTokens: number
  systemTokens: number
  toolsTokens: number
  historyTokens: number
  pendingAppendTokens: number
  outputReserveTokens: number
  protocolOverheadTokens?: number
  safetyMarginTokens: number
  availableTokens: number
  estimateSource: 'heuristic' | 'provider_usage' | 'mixed'
  usageInputTokens?: number
  calibrationRatioPpm?: number
  pressureRatio: number
  triggerReason?: 'none' | 'threshold' | 'single_item_too_large' | 'no_reduction' | 'summary_failed' | 'manual' | 'no_candidates' | 'passes_exhausted' | 'insufficient'
  withinBudget?: boolean
  updatedAt: number
}

export interface ContextBudgetRow {
  key: string
  label: string
  tokens: number
  /** True when the figure comes from exact configuration, false for estimates. */
  exact: boolean
  note?: string
}

export function compactTokenCount(value: number): string {
  if (!Number.isFinite(value) || value < 0) return '—'
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`
  if (value >= 10_000) return `${Math.round(value / 1_000)}K`
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K`
  return String(Math.round(value))
}

/** Itemized rows in the same single derivation as the Host report. */
export function contextBudgetRows(dto: ContextBudgetDto): ContextBudgetRow[] {
  const estimated = dto.estimateSource !== 'provider_usage'
  const estimateNote = estimated ? '估算' : '按供应商 usage 校准'
  return [
    { key: 'system', label: '系统提示', tokens: dto.systemTokens, exact: false, note: estimateNote },
    { key: 'tools', label: '工具定义', tokens: dto.toolsTokens, exact: false, note: estimateNote },
    { key: 'history', label: '历史消息', tokens: dto.historyTokens, exact: false, note: estimateNote },
    { key: 'append', label: '待追加输入', tokens: dto.pendingAppendTokens, exact: false, note: estimateNote },
    { key: 'reserve', label: '输出预留', tokens: dto.outputReserveTokens, exact: true, note: '配置值' },
    { key: 'overhead', label: '协议余量', tokens: dto.protocolOverheadTokens ?? 0, exact: true, note: '固定值' },
    { key: 'margin', label: '误差余量', tokens: dto.safetyMarginTokens, exact: false, note: dto.estimateSource === 'provider_usage' ? '校准后 15%' : '未校准 50%' },
  ]
}

export function estimateSourceLabel(dto: ContextBudgetDto): string {
  switch (dto.estimateSource) {
    case 'provider_usage':
      return '实测校准（供应商 usage）'
    case 'mixed':
      return '估算（有 usage 但无法配对校准）'
    default:
      return '估算（固定比率启发式）'
  }
}

/** Never present an estimate as a measurement: the badge is explicit. */
export function estimateSourceMeasured(dto: ContextBudgetDto): boolean {
  return dto.estimateSource === 'provider_usage'
}

export function usageOccupancyNote(dto: ContextBudgetDto): string | null {
  if (dto.usageInputTokens === undefined) return null
  return '供应商上一轮输入占用（含缓存读取，缓存仍占上下文）'
}

export function triggerReasonLabel(dto: ContextBudgetDto): string {
  switch (dto.triggerReason) {
    case 'threshold':
      return '接近上下文上限，触发压缩'
    case 'single_item_too_large':
      return '单条内容超过窗口，需分页或引用'
    case 'no_reduction':
      return '上次压缩没有收益，有界停止'
    case 'no_candidates':
      return '没有可压缩的候选内容，有界停止'
    case 'summary_failed':
      return '摘要服务失败'
    case 'passes_exhausted':
      return '压缩轮次用尽，有界停止'
    case 'insufficient':
      return '保留必要内容后仍超出上下文容量'
    case 'manual':
      return '手动触发'
    default:
      return '未触发压缩'
  }
}

export function pressurePercent(dto: ContextBudgetDto): number {
  if (!Number.isFinite(dto.pressureRatio)) return 0
  return Math.max(0, Math.min(100, Math.round(dto.pressureRatio * 100)))
}

export function calibrationNote(dto: ContextBudgetDto): string | null {
  if (dto.calibrationRatioPpm === undefined) return null
  const ratio = dto.calibrationRatioPpm / 1_000_000
  return `估算校准系数 ×${ratio.toFixed(2)}（来自本任务最近的供应商 usage）`
}
