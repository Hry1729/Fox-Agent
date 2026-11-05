// Login guidance + permission mode of an external Agent backend (UX batch ①②).
//
// The Host owns every decision (`fox_agent_backend::onboarding`,
// `external_launch`, `host_routing::decide_new_conversation_mode`); this file
// only shapes what the Host returned for display. Nothing here ever carries a
// credential VALUE: the Host returns file names and key names only.

/** `fox_agent_backend::onboarding::CredentialState`. */
export type CredentialState = 'ready' | 'missing_credentials'

export interface ConfigTemplateFile { path: string; content: string }
export interface ConfigTemplate { id: string; title: string; note: string; files: ConfigTemplateFile[] }

/** Result of `agent_backend_credentials` (names only). */
export interface CredentialProbe {
  backend: string
  state: CredentialState
  privateHome: string
  credentialsPresent: boolean
  presentKeys: string[]
  presentFiles: string[]
  requiredKeys: string[]
  requiredFiles: string[]
  templates: ConfigTemplate[]
  /** Settings batch, item 6: what the unified login (Fox「模型服务」) does at the next launch; absent / null without a usable default provider. */
  unifiedLogin?: UnifiedLoginReport | null
  /** UI 1103: whether the Agent's program is installed (the launch's own check); absent from older Hosts. */
  installed?: boolean | null
  source?: { choiceRequired: boolean; nativeAvailable: boolean; reusable: boolean; sourceChanged: boolean; current: string; provider?: string | null; model?: string | null; modelEndpoint: string | null } | null
}

/** `fox_agent_backend::unified_login::UnifiedReport` (names only). */
export interface UnifiedLoginReport {
  backend: string
  outcome: 'generated' | 'unchanged' | 'existing_login' | 'unsupported'
  provider: string
  files: string[]
  userFiles: string[]
  reason: string | null
}

/**
 * Which private HOME the probe / import / "打开文件夹" is about: an existing
 * conversation, or a NEW conversation (backend, project optional) before its
 * first message, so the login card shows before the user can fail once.
 * 没选项目时只带 backend 和 fullAccess。
 */
export type AgentBackendTarget =
  | { conversationId: string }
  | { backend: string; projectId?: string; projectRoot?: string; fullAccess?: boolean }

/** `agent_backend_permission_status`. */
export interface AgentBackendPermissionStatus {
  fullAccessForbidden: boolean
  forbiddenReason: string | null
  engineeringSwitch: boolean
  granted?: boolean
  mode?: 'standard' | 'full_access'
  locked?: boolean
}

/**
 * Fox-level permission of a new external conversation: `standard` (default,
 * no full-access grant) or `full_access`. The Host still accepts the pre-B3
 * name `sandboxed` for `standard`.
 */
export type ExternalPermissionMode = 'standard' | 'full_access'

export const EXTERNAL_PERMISSION_MODE_LABELS: Record<ExternalPermissionMode, string> = {
  standard: '未授予完全访问（默认）',
  full_access: '完全访问',
}

/**
 * Confirmation shown before an Agent's own full-access mode is selected.
 * Fox adds no outer sandbox of its own; this describes the Agent's native
 * consequences, not a Fox sandbox.
 */
export const FULL_ACCESS_RISK_POINTS: readonly string[] = [
  'Fox 不额外包装外部 Agent；这里切换的是该 Agent 自己的完全访问模式。',
  'Codex / DeepSeek 的原生沙箱会随该模式关闭；pi 没有系统级沙箱，边界由 Fox 审批拦截。',
  '后端进程能读写当前用户能访问的文件；超出项目范围时不再由原生沙箱限制。',
]
export const FULL_ACCESS_PRIVATE_HOME_NOTE = '登录和配置仍放在 Fox 私有 HOME 里，不会串到你真实的 HOME。'
export const FULL_ACCESS_LOCK_NOTE = 'Fox 不锁定 Agent 的原生完全访问；会话中可按该 Agent 的方式切换。'

/** Shown before the revoke button acts. */
export const REVOKE_FULL_ACCESS_NOTICE = '切回受限权限会取消当前这一轮、结清待处理的审批，并按该 Agent 的原生方式重建会话；本轮不会重放。'

/** The permission-mode choices of a new external conversation. */
export function externalPermissionModeOptions(status: AgentBackendPermissionStatus | null | undefined): { value: ExternalPermissionMode; label: string; disabled: boolean; reason: string | null }[] {
  const forbidden = Boolean(status?.fullAccessForbidden)
  return [
    { value: 'standard', label: EXTERNAL_PERMISSION_MODE_LABELS.standard, disabled: false, reason: null },
    {
      value: 'full_access',
      label: EXTERNAL_PERMISSION_MODE_LABELS.full_access,
      disabled: forbidden,
      reason: forbidden ? (status?.forbiddenReason || '本机已禁止完全访问') : null,
    },
  ]
}

/** The mode a new conversation is created with: full access only when chosen AND allowed. */
export function effectiveDraftPermissionMode(mode: ExternalPermissionMode, status: AgentBackendPermissionStatus | null | undefined): ExternalPermissionMode {
  return mode === 'full_access' && !status?.fullAccessForbidden ? 'full_access' : 'standard'
}

/** Legacy lock-chip text; the bar now shows the Agent's native picker. */
export function permissionChipLabel(status: AgentBackendPermissionStatus | null | undefined): string | null {
  if (!status?.mode) return null
  return '隔离：Fox 不额外包装（Agent 原生）'
}
