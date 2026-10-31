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

/** Why 模型服务 could not log this backend in, for the missing-login card (null when it can, or is not configured). */
export function unifiedLoginNote(probe: CredentialProbe | null | undefined): string | null {
  const unified = probe?.unifiedLogin
  if (!unified || unified.outcome !== 'unsupported') return null
  return `没能用“模型服务”的供应商「${unified.provider}」自动登录：${unified.reason ?? '原因未知'}`
}

/** One whitelisted item of the real HOME (`agent_backend_import_candidates`). */
export interface ImportItem {
  id: string
  kind: 'file' | 'env_key'
  /** File name or key name; never a value. */
  name: string
  /** Destination relative to the private HOME. */
  destination: string
}

/** A project whose private HOME already had some items (kept, not copied). */
export interface ImportSkip { project: string; label: string; items: string[] }

/**
 * Where the overwritten items of one project were backed up. `directory` is
 * relative to the private HOME (`.fox-credential-backups/<stamp>`), the same
 * form as the audit; the Host never sends the absolute path (it contains the
 * user's SID / profile).
 */
export interface ImportBackup { project: string; label: string; directory: string; items: string[] }

/** Folder (under the private HOME) that holds overwritten logins. */
export const CREDENTIAL_BACKUP_DIR = '.fox-credential-backups'

function hostIsWindows(): boolean {
  return typeof navigator !== 'undefined' && /windows/i.test(navigator.userAgent || '')
}

/**
 * The backup location as the receipt shows it: relative to the private HOME
 * (`.fox-credential-backups\<时间戳>` on Windows), never an absolute path
 * even if an older Host sent one.
 */
export function backupDirectoryLabel(directory: string, windows = hostIsWindows()): string {
  const normalized = directory.replace(/\\/g, '/')
  const at = normalized.lastIndexOf(`${CREDENTIAL_BACKUP_DIR}/`)
  const relative = at >= 0 ? normalized.slice(at) : `${CREDENTIAL_BACKUP_DIR}/${normalized.split('/').filter(Boolean).pop() ?? ''}`
  return windows ? relative.replace(/\//g, '\\') : relative
}

/** Audit record returned by `agent_backend_import_credentials` (names only). */
export interface ImportReport {
  backend: string
  /** Private HOMEs targeted. */
  homes: number
  /** Private HOMEs where something was written (older Hosts: absent). */
  homesWritten?: number
  /** "覆盖已有登录" was chosen. */
  overwrite?: boolean
  files: string[]
  keys: string[]
  skipped: string[]
  skippedExisting?: ImportSkip[]
  backups?: ImportBackup[]
}

/**
 * Which private HOME the probe / import / "打开文件夹" is about: an existing
 * conversation, or a NEW conversation (backend + project) before its first
 * message, so the login card shows before the user can fail once.
 */
export type AgentBackendTarget =
  | { conversationId: string }
  | { backend: string; projectId?: string; projectRoot?: string; fullAccess?: boolean }

/**
 * The probe target of the shown backend: the open conversation when it is
 * locked to an external backend; else, for a new conversation with an
 * external backend picked, its project (id or root) and draft mode; null for
 * Fox, or a draft with no project yet (the Host keys private HOMEs by project).
 */
export function credentialProbeTarget(input: {
  lockedBackend: string | null
  conversationId: string | null
  draftBackend: string
  draftProjectId?: string | null
  draftProjectRoot?: string | null
  draftFullAccess?: boolean
}): AgentBackendTarget | null {
  if (input.lockedBackend) {
    return input.lockedBackend !== 'fox' && input.conversationId ? { conversationId: input.conversationId } : null
  }
  if (input.draftBackend === 'fox') return null
  const projectId = input.draftProjectId || undefined
  const projectRoot = input.draftProjectRoot || undefined
  if (!projectId && !projectRoot) return null
  return {
    backend: input.draftBackend,
    ...(projectId ? { projectId } : {}),
    ...(projectRoot ? { projectRoot } : {}),
    fullAccess: Boolean(input.draftFullAccess),
  }
}

/** Stable key of a target (effect dependency). */
export function credentialProbeTargetKey(target: AgentBackendTarget | null): string {
  return target ? JSON.stringify(target) : ''
}

/** How long the import receipt stays on the bar after the card is gone. */
export const IMPORT_RECEIPT_VISIBLE_MS = 30_000

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
 * Fox no longer adds an outer SRT layer; this describes the Agent's native
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

export interface CredentialCardModel {
  kind: 'missing' | 'source' | 'ready'
  title: string
  description: string
  privateHome: string
  requiredKeys: string[]
  requiredFiles: string[]
  presentFiles: string[]
  templates: ConfigTemplate[]
  /** Offer "从真实 HOME 导入" (missing login only). */
  offerImport: boolean
}

/**
 * The card for a probe, or null when the backend is ready: a missing login is
 * fixed in the private HOME (open folder / templates / import); a Pi probe
 * may ask for its configuration source.
 */
export function credentialCardModel(probe: CredentialProbe | null | undefined, _permission?: AgentBackendPermissionStatus | null, showReady = false): CredentialCardModel | null {
  if (!probe) return null
  if (probe.source?.choiceRequired || (showReady && probe.state === 'ready')) {
    return {
      kind: probe.source?.choiceRequired ? 'source' : 'ready',
      title: probe.source?.choiceRequired ? '选择 Pi 配置来源' : 'Agent 配置来源',
      description: probe.source?.sourceChanged ? '原生 Pi 配置已变化。请重新确认导入，或选择 Fox 模型服务。'
        : probe.source?.choiceRequired ? '发现本机原生 Pi 配置。确认导入后，当前项目可使用相同的模型和登录。'
        : probe.source?.current === 'native' || probe.source?.current === 'reused_native' ? '当前使用项目中的原生 Agent 配置。可在这里重新导入。' : '当前配置已就绪。可在这里导入原生 Agent 配置。',
      privateHome: probe.privateHome, requiredKeys: [], requiredFiles: [], presentFiles: probe.presentFiles, templates: [], offerImport: true,
    }
  }
  if (probe.state === 'ready') return null
  const note = unifiedLoginNote(probe)
  const protocol = probe.backend === 'codex' ? 'OpenAI Responses' : probe.backend === 'deepseek' ? 'Anthropic Messages' : '兼容协议'
  return {
    kind: 'missing',
    title: '这个后端还没有登录',
    description: `${note ? `${note}。` : `请在“设置 → 模型服务”添加 ${protocol} 配置并指定给此 Agent，保存 Key 后重试；也可以手动登录：`}外部后端只读取 Fox 为本项目准备的私有 HOME，你真实 HOME 里的登录在这里用不上。把需要的文件放进下面的文件夹，或从真实 HOME 导入。`,
    privateHome: probe.privateHome,
    requiredKeys: probe.requiredKeys,
    requiredFiles: probe.requiredFiles,
    presentFiles: probe.presentFiles,
    templates: probe.templates,
    offerImport: true,
  }
}

/** Rows of the import confirmation: names only (the Host never sends values). */
export function importConfirmationRows(items: readonly ImportItem[]): { id: string; label: string; detail: string }[] {
  return items.map((item) => ({
    id: item.id,
    label: item.kind === 'env_key' ? `${item.name}（${item.destination} 中的一行）` : item.name,
    detail: `复制到私有 HOME 的 ${item.destination}`,
  }))
}

const baseName = (path: string) => path.split('/').pop() ?? path

/**
 * Receipt lines after an import (names only, no value). Shown OUTSIDE the
 * card: a successful import turns the probe ready and the card disappears.
 */
export function importReceiptLines(report: ImportReport, options: { windows?: boolean } = {}): string[] {
  const names = [...report.files.map(baseName), ...report.keys]
  const written = report.homesWritten ?? (names.length ? report.homes : 0)
  const lines: string[] = []
  if (names.length && written > 0) {
    const where = report.homes > 1 ? ` ${written} 个项目的私有 HOME` : '本项目的私有 HOME'
    lines.push(`已复制到${where}：${names.join('、')}`)
  } else {
    lines.push('没有复制任何内容')
  }
  const kept = report.skippedExisting ?? []
  if (kept.length) {
    const projects = kept.map((skip) => `${skip.label}（${skip.items.map(baseName).join('、')}）`)
    lines.push(`已有登录，未覆盖 ${kept.length} 个项目：${projects.join('、')}`)
  }
  for (const backup of report.backups ?? []) {
    lines.push(`已覆盖 ${backup.label} 的 ${backup.items.map(baseName).join('、')}，原文件备份在私有 HOME 下的 ${backupDirectoryLabel(backup.directory, options.windows)}`)
  }
  if (report.skipped.length) lines.push(`${report.skipped.length} 项已不在可导入列表中，未复制`)
  return lines
}

/** One-line receipt after an import (names only). */
export function importReceipt(report: ImportReport, options: { windows?: boolean } = {}): string {
  return importReceiptLines(report, options).join('；')
}
