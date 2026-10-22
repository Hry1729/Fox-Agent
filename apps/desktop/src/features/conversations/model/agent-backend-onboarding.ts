// Login guidance + permission mode of an external Agent backend (UX batch ①②).
//
// The Host owns every decision (`fox_agent_backend::onboarding`,
// `external_launch`, `host_routing::decide_new_conversation_mode`); this file
// only shapes what the Host returned for display. Nothing here ever carries a
// credential VALUE: the Host returns file names and key names only.

/** `fox_agent_backend::onboarding::CredentialState`. */
export type CredentialState = 'ready' | 'missing_credentials' | 'loopback_requires_full_access'

export interface ConfigTemplateFile { path: string; content: string }
export interface ConfigTemplate { id: string; title: string; note: string; files: ConfigTemplateFile[] }

/** Result of `agent_backend_credentials` (names only). */
export interface CredentialProbe {
  backend: string
  mode: 'sandboxed' | 'full_access' | string
  state: CredentialState
  privateHome: string
  credentialsPresent: boolean
  loopbackBlocked: boolean
  presentKeys: string[]
  presentFiles: string[]
  requiredKeys: string[]
  requiredFiles: string[]
  templates: ConfigTemplate[]
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

/** Where the overwritten items of one project were backed up. */
export interface ImportBackup { project: string; label: string; directory: string; items: string[] }

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
  mode?: 'sandboxed' | 'full_access'
  locked?: boolean
}

export type ExternalPermissionMode = 'sandboxed' | 'full_access'

export const EXTERNAL_PERMISSION_MODE_LABELS: Record<ExternalPermissionMode, string> = {
  sandboxed: '沙箱（默认）',
  full_access: '完全访问',
}

/**
 * Risk confirmation shown before a new conversation is created with full
 * access. Three points (no srt, no directory boundary, the process can read
 * and write anything the user can), then where logins live.
 */
export const FULL_ACCESS_RISK_POINTS: readonly string[] = [
  '不套沙箱（srt）：后端进程直接在你的账户下运行。',
  '不承诺目录边界：授权的项目文件夹之外，Fox 也不会拦住它。',
  '后端进程能读写当前用户能访问的任何文件。',
]
export const FULL_ACCESS_PRIVATE_HOME_NOTE = '登录和配置仍放在 Fox 私有 HOME 里，不会串到你真实的 HOME。'
export const FULL_ACCESS_LOCK_NOTE = '选定后与后端一起锁定；会话中只能撤销，不能再开启。'

/** Shown before the revoke button acts. */
export const REVOKE_FULL_ACCESS_NOTICE = '撤销完全访问会取消当前这一轮、结清待处理的审批，并在沙箱中重新启动外部后端；本轮不会重放。'

/** The permission-mode choices of a new external conversation. */
export function externalPermissionModeOptions(status: AgentBackendPermissionStatus | null | undefined): { value: ExternalPermissionMode; label: string; disabled: boolean; reason: string | null }[] {
  const forbidden = Boolean(status?.fullAccessForbidden)
  return [
    { value: 'sandboxed', label: EXTERNAL_PERMISSION_MODE_LABELS.sandboxed, disabled: false, reason: null },
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
  return mode === 'full_access' && !status?.fullAccessForbidden ? 'full_access' : 'sandboxed'
}

/** Lock chip text of an open external conversation. */
export function permissionChipLabel(status: AgentBackendPermissionStatus | null | undefined): string | null {
  if (!status?.mode) return null
  return `权限：${status.mode === 'full_access' ? '完全访问' : '沙箱'}（已锁定）`
}

/** Whether the "撤销完全访问" action applies (only a user grant can be revoked). */
export function canRevokeFullAccess(status: AgentBackendPermissionStatus | null | undefined): boolean {
  return Boolean(status?.granted)
}

export interface CredentialCardModel {
  kind: 'missing' | 'loopback'
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
 * The card for a probe, or null when the backend is ready. The two problems
 * are different cards with different actions: a missing login is fixed in the
 * private HOME (open folder / templates / import); a loopback gateway in the
 * sandbox needs full access (chosen when creating a conversation) or the
 * official endpoint.
 */
export function credentialCardModel(probe: CredentialProbe | null | undefined, permission?: AgentBackendPermissionStatus | null): CredentialCardModel | null {
  if (!probe || probe.state === 'ready') return null
  if (probe.state === 'loopback_requires_full_access') {
    const forbidden = Boolean(permission?.fullAccessForbidden)
    return {
      kind: 'loopback',
      title: '本机网关在沙箱模式下不可用',
      description: forbidden
        ? 'DeepSeek 的 base URL 指向本机回环地址（如 Magpie），沙箱模式会在启动前拒绝。本机已禁止完全访问，请把 base URL 改成官方 https://api.deepseek.com/anthropic。'
        : 'DeepSeek 的 base URL 指向本机回环地址（如 Magpie），沙箱模式会在启动前拒绝。请新建会话并选择“完全访问”，或把 base URL 改成官方 https://api.deepseek.com/anthropic。',
      privateHome: probe.privateHome,
      requiredKeys: [],
      requiredFiles: probe.presentFiles,
      presentFiles: probe.presentFiles,
      templates: probe.templates.filter((template) => template.id === 'deepseek_official'),
      offerImport: false,
    }
  }
  return {
    kind: 'missing',
    title: '这个后端还没有登录',
    description: '外部后端只读取 Fox 为本项目准备的私有 HOME，你真实 HOME 里的登录在这里用不上。把需要的文件放进下面的文件夹，或从真实 HOME 导入。',
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
export function importReceiptLines(report: ImportReport): string[] {
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
    lines.push(`已覆盖 ${backup.label} 的 ${backup.items.map(baseName).join('、')}，原文件备份在 ${backup.directory}`)
  }
  if (report.skipped.length) lines.push(`${report.skipped.length} 项已不在可导入列表中，未复制`)
  return lines
}

/** One-line receipt after an import (names only). */
export function importReceipt(report: ImportReport): string {
  return importReceiptLines(report).join('；')
}
