// Per-agent approval mode list (plan C2, composer batch 2026-10-31).
//
// The list, its names (full and two-character short), which option needs
// the first-time confirmation and when a change applies all come from the
// Host (`agent_backend_native_permissions` / `agent_backend_approval_modes`,
// built by `fox_agent_backend::native_permission`): each Agent lists its own
// modes one to one (Codex 请求审批 / 帮我批准 / 完全访问权限, dsh 仅可查看 /
// 工作区完全修改 / 完全权限, pi and Fox 只读 / 询问 / 自动 / 完全访问). This
// file only turns the Host's view into picker rows, says when a change
// applies, and decides which confirmation a pick needs; the Host refuses
// anything the UI should not have offered.
import { FULL_ACCESS_RISK_POINTS, REVOKE_FULL_ACCESS_NOTICE } from './agent-backend-onboarding'
import { AGENT_BACKEND_LABELS, type AgentBackendId } from './agent-backend'

export type NativeSwitchTiming = 'live' | 'next_turn_restart' | 'host_answered' | 'next_tool_call'

/** Fox's four tiers (Host `native_permission::FoxTier`). */
export type FoxPermissionTier = 'read_only' | 'ask' | 'auto' | 'full_access'

export interface NativePermissionOption {
  id: string
  /** The Fox tier this option is; absent from older Hosts. */
  tier?: FoxPermissionTier
  label: string
  /** Two-character name on the composer button (older Hosts: derived from `label`). */
  shortLabel?: string
  description: string
  /** The Agent's own name of what is configured. */
  native: string
  /** This option is the Agent's own full-access mode. */
  fullAccess: boolean
  /** Picking it needs the first-time second confirmation (older Hosts: `fullAccess`). */
  needsConfirmation?: boolean
  /** Offered in the picker; `false` = an older build's mode, shown only while current. */
  listed?: boolean
  disabledReason: string | null
}

export interface NativePermissionCatalog {
  backend: string
  /** The Agent's display name (Host `native_permission::display_name`: Codex / DeepSeek / pi). */
  displayName?: string
  options: NativePermissionOption[]
  defaultMode: string
  switchTiming: NativeSwitchTiming
  switchNote: string
}

/** `agent_backend_native_permissions` / `agent_backend_native_permission_set`. */
export interface NativePermissionView {
  catalog: NativePermissionCatalog
  current: string
  /** The Fox tier of `current` (carried when a new conversation switches Agent). */
  currentTier?: FoxPermissionTier | null
  /** The mode the live process runs in (null: not running). */
  running?: string | null
  /** The stored choice is not yet what the live process runs in (dsh: next turn). */
  pending?: boolean
  /** The non-full-access choice (the same as `current` unless in full access). */
  restrictedMode?: string
  fullAccess: boolean
  fullAccessGranted: boolean
  fullAccessForbidden: boolean
  /** A new conversation's list (no conversation yet). */
  draft: boolean
  /** Why the full-access option cannot be picked now (mid-session). */
  fullAccessLockedReason: string | null
  /** Set only: how the change reached the session. */
  applied?: 'not_running' | 'applied' | 'next_turn' | 'failed' | string
  appliedError?: string
  /**
   * The conversation's (or draft's) project already confirmed full access
   * once (plan C2: the same first-time second confirmation as a Fox
   * conversation). Absent from older Hosts: confirm every time.
   */
  projectFullAccessConfirmed?: boolean
}

export interface NativePermissionRequest {
  conversationId?: string | null
  backend?: string | null
  mode?: string
  /** 设置: remember the Agent's default non-full-access choice only. */
  defaultOnly?: boolean
  /** A new conversation's project (the first full access per project is confirmed once). */
  projectId?: string | null
  /** The user confirmed the full-access warning. */
  confirmFullAccess?: boolean
  /**
   * A new conversation, no `mode`: its Agent was switched (or it was
   * opened). `''` = fresh (the Agent's default); a Fox tier = the previous
   * choice's tier, mapped by the Host. Full access is never carried.
   */
  carryTier?: FoxPermissionTier | ''
}

/** `agent_backend_approval_modes`: every backend's list, Fox included. */
export interface ApprovalModeLists {
  backends: NativePermissionCatalog[]
}

export interface NativePermissionRow {
  value: string
  label: string
  shortLabel: string
  tier: FoxPermissionTier | null
  /** Picking it needs the first-time second confirmation. */
  needsConfirmation: boolean
  title: string
  disabled: boolean
  reason: string | null
  fullAccess: boolean
}

/** dsh: a change waits for the restart at the next turn. */
export const NEXT_TURN_RESTART_BADGE = '下一轮重启生效'
/** Codex: `session/set_mode` on the live session. */
export const LIVE_SWITCH_BADGE = '切换立即生效'
/** pi: Fox's approval engine answers by the new tier. */
export const HOST_ANSWERED_BADGE = '下一次请求起生效'
/** Fox / PiNative: from the next tool call of the running turn. */
export const NEXT_TOOL_CALL_BADGE = '下一次工具调用起生效'

/**
 * Picker rows in the Agent's own names. Disabled rows keep only the Host's
 * reason (e.g. Codex「帮我批准」off OpenAI, or FORBIDDEN). The Agent's own
 * full access is selectable in place; it is not a Fox-level mode.
 * `restrictedOnly` (设置) leaves the full-access option out of the default
 * permission rows.
 */
export function nativePermissionRows(view: NativePermissionView | null | undefined, options: { restrictedOnly?: boolean } = {}): NativePermissionRow[] {
  if (!view) return []
  return catalogRows(view.catalog, view.current)
    .filter((row) => !(options.restrictedOnly && row.fullAccess))
}

/** Two characters for the composer button. */
export function approvalShortLabel(option: Pick<NativePermissionOption, 'label' | 'shortLabel'>): string {
  return option.shortLabel?.trim() || Array.from(option.label).slice(0, 2).join('')
}

/**
 * The rows of a Host list: the listed options, plus an unlisted one (an
 * older build's mode) only while it is `current`, so the picker never hides
 * what really runs.
 */
export function catalogRows(catalog: NativePermissionCatalog, current: string | null | undefined): NativePermissionRow[] {
  return catalog.options
    .filter((option) => option.listed !== false || option.id === current)
    .map((option) => {
      const legacy = option.listed === false
      const reason = option.disabledReason ?? (legacy ? '已不再提供，切换后不能再选回' : null)
      return {
        value: option.id,
        label: option.label,
        shortLabel: approvalShortLabel(option),
        tier: option.tier ?? null,
        needsConfirmation: option.needsConfirmation ?? option.fullAccess,
        title: [option.description, option.native, reason].filter(Boolean).join(' · '),
        disabled: Boolean(option.disabledReason) || legacy,
        reason,
        fullAccess: option.fullAccess,
      }
    })
}

/** The small note on the composer button: the choice waits for the next turn. */
export const PENDING_NEXT_TURN_NOTE = '下一轮生效'
export function nativePendingNote(view: NativePermissionView | null | undefined): string | null {
  return view && !view.draft && view.pending ? PENDING_NEXT_TURN_NOTE : null
}

/** The Fox tier of a Fox conversation's composer mode (`allow` = 自动). */
export function foxTierOfAccessMode(mode: string | null | undefined): FoxPermissionTier {
  switch (mode) {
    case 'read_only': return 'read_only'
    case 'allow': return 'auto'
    case 'full_access': return 'full_access'
    default: return 'ask'
  }
}

/** The Host's list for `backend` out of `agent_backend_approval_modes`. */
export function approvalCatalogFor(lists: ApprovalModeLists | null | undefined, backend: string): NativePermissionCatalog | null {
  return lists?.backends.find((catalog) => catalog.backend === backend) ?? null
}

/** The note shown next to the picker: when a change applies (plan C2). */
export function nativeSwitchBadge(view: NativePermissionView | null | undefined): string | null {
  if (!view || view.draft) return null
  switch (view.catalog.switchTiming) {
    case 'next_turn_restart': return NEXT_TURN_RESTART_BADGE
    case 'live': return LIVE_SWITCH_BADGE
    case 'host_answered': return HOST_ANSWERED_BADGE
    case 'next_tool_call': return NEXT_TOOL_CALL_BADGE
    default: return null
  }
}

export type NativePermissionChange =
  | { kind: 'none' }
  | { kind: 'refused'; reason: string }
  /** The Agent's native full access: the risk confirmation. */
  | { kind: 'confirm_full_access'; points: readonly string[] }
  /** Leaving the Agent's native full access: one confirmation. */
  | { kind: 'confirm_revoke'; notice: string }
  | { kind: 'apply' }

/** What picking `mode` needs before the Host is asked. */
export function nativePermissionChange(view: NativePermissionView | null | undefined, mode: string): NativePermissionChange {
  if (!view || mode === view.current) return { kind: 'none' }
  const row = nativePermissionRows(view).find((item) => item.value === mode)
  if (!row) return { kind: 'refused', reason: '没有这个权限选项' }
  if (row.disabled) return { kind: 'refused', reason: row.reason ?? '不可用' }
  if (row.needsConfirmation) {
    // The first full access in a project is confirmed once; the project
    // remembers it afterwards (the Host enforces the same rule).
    if (view.projectFullAccessConfirmed) return { kind: 'apply' }
    return { kind: 'confirm_full_access', points: FULL_ACCESS_RISK_POINTS }
  }
  if (!view.draft && view.fullAccess) {
    return { kind: 'confirm_revoke', notice: REVOKE_FULL_ACCESS_NOTICE }
  }
  return { kind: 'apply' }
}

/**
 * The Agent's name in UI text: the Host's `displayName`, else the backend
 * bar's label for the id (older Hosts), else the id itself.
 */
export function nativeBackendName(view: NativePermissionView | null | undefined): string {
  const catalog = view?.catalog
  if (!catalog) return ''
  return catalog.displayName?.trim() || AGENT_BACKEND_LABELS[catalog.backend as AgentBackendId] || catalog.backend
}

/** One line after a mid-session switch. */
export function nativeAppliedNotice(view: NativePermissionView | null | undefined): string | null {
  switch (view?.applied) {
    case 'applied': return view.catalog.switchTiming === 'host_answered' ? '已切换，从下一次请求起生效' : '已切换，当前会话立即生效'
    case 'next_turn': return `已保存，下一轮开始时重启 ${nativeBackendName(view)} 生效`
    case 'failed': return `切换未生效，下一轮会重试${view.appliedError ? `：${view.appliedError}` : ''}`
    default: return null
  }
}
