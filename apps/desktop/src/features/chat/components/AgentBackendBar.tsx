// Four-agent plan v0.3.1, Host wiring first batch (P6 minimal UI):
// * a new conversation can pick its outer Agent backend; while the Host switch
//   (FOX_EXTERNAL_BACKENDS) keeps external backends off, the picker is disabled
//   and says why;
// * an existing conversation shows its frozen backend as locked, and a
//   read-only notice when the Host would refuse to start its runtime.
//
// Display only: every rule is enforced by the Host (`host_routing`), so a
// stale or tampered UI can never widen what runs.
import { Lock } from 'lucide-react'
import { useState, type ReactNode } from 'react'
import {
  canRevokeFullAccess,
  externalPermissionModeOptions,
  FULL_ACCESS_LOCK_NOTE,
  FULL_ACCESS_PRIVATE_HOME_NOTE,
  FULL_ACCESS_RISK_POINTS,
  permissionChipLabel,
  REVOKE_FULL_ACCESS_NOTICE,
  type AgentBackendPermissionStatus,
  type ExternalPermissionMode,
} from '@/features/conversations/model/agent-backend-onboarding'
import {
  AGENT_BACKEND_IDS,
  AGENT_BACKEND_LABELS,
  agentBackendAccess,
  agentBackendLockNotice,
  agentBackendPickerDisabledReason,
  canPickAgentBackend,
  isAgentBackendId,
  agentBackendCapabilityChips,
  type AgentBackendCapabilities,
  type AgentBackendId,
  type AgentBackendStatus,
} from '@/features/conversations/model/agent-backend'
import './agent-backend-bar.css'

/** The four effective capability dimensions (external backend wiring batch). */
function CapabilityChips({ capabilities }: { capabilities: AgentBackendCapabilities | null }) {
  const chips = agentBackendCapabilityChips(capabilities)
  if (chips.length === 0) return null
  return (
    <span className="fox-agent-backend-capabilities" aria-label="后端能力声明">
      {chips.map((chip) => (
        <span key={chip.key} className="fox-agent-backend-capability" data-dimension={chip.key} title={chip.title}>{chip.label}</span>
      ))}
      {capabilities?.warning && <span className="fox-agent-backend-notice" role="note">{capabilities.warning}</span>}
    </span>
  )
}

/** UX batch ②: the permission mode of a NEW external conversation (sandbox by default; full access after an explicit risk confirmation; disabled and explained when forbidden). */
function DraftPermissionPicker({ permission, mode, onChange }: {
  permission: AgentBackendPermissionStatus | null
  mode: ExternalPermissionMode
  onChange: (mode: ExternalPermissionMode) => void
}) {
  const [confirming, setConfirming] = useState(false)
  const options = externalPermissionModeOptions(permission)
  const forbidden = options.find((option) => option.value === 'full_access')?.reason ?? null
  return (
    <>
      <label className="fox-agent-backend-picker" title={forbidden ?? FULL_ACCESS_LOCK_NOTE}>
        <span>权限</span>
        <select
          aria-label="新会话的权限模式"
          value={forbidden ? 'sandboxed' : mode}
          onChange={(event) => {
            const next = event.target.value as ExternalPermissionMode
            if (next === 'full_access') {
              if (!forbidden) setConfirming(true)
              return
            }
            setConfirming(false)
            onChange('sandboxed')
          }}
        >
          {options.map((option) => (
            <option key={option.value} value={option.value} disabled={option.disabled} title={option.reason ?? undefined}>
              {option.label}{option.disabled ? '（已禁止）' : ''}
            </option>
          ))}
        </select>
      </label>
      {forbidden && <span className="fox-agent-backend-notice">{forbidden}</span>}
      {confirming && (
        <div className="fox-agent-backend-risk" role="alertdialog" aria-label="完全访问风险确认">
          <strong>使用完全访问？</strong>
          <ul>{FULL_ACCESS_RISK_POINTS.map((point) => <li key={point}>{point}</li>)}</ul>
          <p>{FULL_ACCESS_PRIVATE_HOME_NOTE}</p>
          <p>{FULL_ACCESS_LOCK_NOTE}</p>
          <button type="button" onClick={() => { setConfirming(false); onChange('full_access') }}>我已了解，使用完全访问</button>
          <button type="button" onClick={() => { setConfirming(false); onChange('sandboxed') }}>取消</button>
        </div>
      )}
    </>
  )
}

/** UX batch ②: lock chip of the permission mode + "撤销完全访问" (warns that the backend restarts). */
function LockedPermission({ permission, onRevoke }: { permission: AgentBackendPermissionStatus | null; onRevoke?: () => void }) {
  const [confirming, setConfirming] = useState(false)
  const label = permissionChipLabel(permission)
  if (!label) return null
  return (
    <>
      <span className="fox-agent-backend-chip" data-permission={permission?.mode} title={FULL_ACCESS_LOCK_NOTE}>
        <Lock size={11} aria-hidden="true" />
        {label}
      </span>
      {onRevoke && canRevokeFullAccess(permission) && (confirming
        ? (
          <span className="fox-agent-backend-risk" role="alertdialog" aria-label="撤销完全访问">
            {REVOKE_FULL_ACCESS_NOTICE}
            <button type="button" onClick={() => { setConfirming(false); onRevoke() }}>撤销并重启到沙箱</button>
            <button type="button" onClick={() => setConfirming(false)}>取消</button>
          </span>
        )
        : <button type="button" className="fox-agent-backend-revoke" onClick={() => setConfirming(true)}>撤销完全访问</button>)}
    </>
  )
}

export function AgentBackendBar({ status, lockedBackend, draftBackend, onDraftBackendChange, refusal = null, capabilities = null, permission = null, draftPermissionMode = 'sandboxed', onDraftPermissionModeChange, onRevokeFullAccess, credentialCard = null }: {
  status: AgentBackendStatus | null
  /** The frozen backend of the open conversation; null for a new conversation. */
  lockedBackend: AgentBackendId | null
  draftBackend: AgentBackendId
  onDraftBackendChange: (backend: AgentBackendId) => void
  /**
   * The Host's refusal of the last send in this view (`agent_backend_*`).
   * Shown as the ONE notice of the bar, replacing the generic lock / picker
   * notice, so a refused send never produces two messages.
   */
  refusal?: string | null
  /** Effective capabilities of the shown external backend (locked or picked); null for Fox. */
  capabilities?: AgentBackendCapabilities | null
  /** UX batch ②: permission mode / lock / FORBIDDEN from the Host. */
  permission?: AgentBackendPermissionStatus | null
  draftPermissionMode?: ExternalPermissionMode
  onDraftPermissionModeChange?: (mode: ExternalPermissionMode) => void
  onRevokeFullAccess?: () => void
  /** UX batch ①: login guidance card of the open external conversation. */
  credentialCard?: ReactNode
}) {
  const refusalNotice = refusal
    ? <span className="fox-agent-backend-notice fox-agent-backend-refusal" role="alert">{refusal}</span>
    : null
  if (lockedBackend) {
    const access = agentBackendAccess(lockedBackend, status)
    const notice = agentBackendLockNotice(lockedBackend, access, status?.switchEnv)
    // A Fox conversation keeps today's composer exactly: no extra row.
    if (lockedBackend === 'fox') return null
    return (
      <div className="fox-agent-backend-bar" data-access={access} role="status">
        <span className="fox-agent-backend-chip" title="会话创建时已确定，之后不可更换">
          <Lock size={11} aria-hidden="true" />
          后端：{AGENT_BACKEND_LABELS[lockedBackend]}（已锁定）
        </span>
        {access === 'runnable' && <LockedPermission permission={permission} onRevoke={onRevokeFullAccess} />}
        {refusalNotice ?? (notice && <span className="fox-agent-backend-notice">{notice}</span>)}
        {access === 'runnable' && <CapabilityChips capabilities={capabilities} />}
        {access === 'runnable' && credentialCard}
      </div>
    )
  }

  const disabledReason = agentBackendPickerDisabledReason(status)
  const value = canPickAgentBackend(draftBackend, status) ? draftBackend : 'fox'
  const option = status?.options.find((item) => item.backend === value)
  const pickedNotice = value !== 'fox' && option && !option.wired ? option.reason : null
  // With the switch off the control stays visible but disabled, with the
  // reason next to it (plan 6.5: unsupported settings are disabled and
  // explained, never silently hidden or swapped).
  return (
    <div className="fox-agent-backend-bar" data-access="draft">
      <label className="fox-agent-backend-picker" title={disabledReason ?? '新会话使用的 Agent 后端，创建后不可更换'}>
        <span>Agent 后端</span>
        <select
          aria-label="新会话的 Agent 后端"
          value={value}
          disabled={Boolean(disabledReason)}
          onChange={(event) => {
            const next = event.target.value
            if (isAgentBackendId(next) && canPickAgentBackend(next, status)) onDraftBackendChange(next)
          }}
        >
          {AGENT_BACKEND_IDS.map((backend) => {
            const entry = status?.options.find((item) => item.backend === backend)
            const enabled = backend === 'fox' || Boolean(entry?.enabled)
            return (
              <option key={backend} value={backend} disabled={!enabled} title={entry?.reason ?? undefined}>
                {AGENT_BACKEND_LABELS[backend]}{entry?.experimental ? '（实验）' : ''}{enabled ? '' : '（未启用）'}
              </option>
            )
          })}
        </select>
      </label>
      {refusalNotice ?? <>
        {disabledReason && <span className="fox-agent-backend-notice">{disabledReason}</span>}
        {pickedNotice && <span className="fox-agent-backend-notice">{pickedNotice}</span>}
        {value !== 'fox' && onDraftPermissionModeChange && <DraftPermissionPicker permission={permission} mode={draftPermissionMode} onChange={onDraftPermissionModeChange} />}
        {value !== 'fox' && <CapabilityChips capabilities={capabilities} />}
      </>}
    </div>
  )
}
