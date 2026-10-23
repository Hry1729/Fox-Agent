// Per-agent native permission picker on the backend bar.
//
// * Lists ONLY what the shown Agent supports, in its own names (Codex:
//  「请求审批 / 帮我批准 / 完全访问权限」; dsh / pi their own lists); a row the
//   Host greys (Codex「帮我批准」off OpenAI, FORBIDDEN, full access
//   mid-session) shows its reason inline next to the picker.
// * dsh: a mid-session change is marked「下一轮重启生效」.
// * Picking full access in a new conversation goes through the existing risk
//   confirmation; mid-session full access is never offered (only revoke,
//   with the existing revoke confirmation).
// * Keyed by target: a late answer for another conversation / backend is
//   dropped. Every rule is enforced again by the Host.
import { useCallback, useEffect, useRef, useState } from 'react'
import { FULL_ACCESS_PRIVATE_HOME_NOTE } from '@/features/conversations/model/agent-backend-onboarding'
import {
  nativeAppliedNotice,
  nativePermissionChange,
  nativePermissionRows,
  nativeSwitchBadge,
  type NativePermissionChange,
  type NativePermissionRequest,
  type NativePermissionView,
} from '@/features/conversations/model/agent-native-permission'

export interface NativePermissionClient {
  agentBackendNativePermissions: (request: NativePermissionRequest) => Promise<NativePermissionView>
  agentBackendNativePermissionSet: (request: NativePermissionRequest & { mode: string }) => Promise<NativePermissionView>
}

function errorText(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause)
}

export function AgentNativePermissionPicker({ client, conversationId, backend, tick = 0, onChanged }: {
  client: NativePermissionClient
  /** The open conversation; null for a new conversation of `backend`. */
  conversationId: string | null
  backend: string
  tick?: number
  onChanged?: (view: NativePermissionView) => void
}) {
  const key = JSON.stringify([conversationId, backend])
  const [state, setState] = useState<{ key: string; view: NativePermissionView } | null>(null)
  const [pending, setPending] = useState<{ key: string; mode: string; change: NativePermissionChange } | null>(null)
  const [message, setMessage] = useState<{ key: string; text: string; error: boolean } | null>(null)
  const [busy, setBusy] = useState(false)
  const sequence = useRef(0)

  const request = useCallback((): NativePermissionRequest => (conversationId ? { conversationId } : { backend }), [conversationId, backend])

  useEffect(() => {
    const id = ++sequence.current
    setState((current) => (current && current.key === key ? current : null))
    setPending(null)
    setMessage((current) => (current && current.key === key ? current : null))
    client.agentBackendNativePermissions(request())
      .then((view) => { if (id === sequence.current) setState({ key, view }) })
      .catch(() => { if (id === sequence.current) setState(null) })
  }, [client, key, tick, request])

  const view = state && state.key === key ? state.view : null
  const rows = nativePermissionRows(view)
  if (!view || rows.length === 0) return null

  const apply = async (mode: string) => {
    setBusy(true)
    const id = sequence.current
    try {
      const next = await client.agentBackendNativePermissionSet({ ...request(), mode })
      if (id !== sequence.current) return
      setState({ key, view: next })
      const notice = nativeAppliedNotice(next)
      setMessage(notice ? { key, text: notice, error: next.applied === 'failed' } : null)
      onChanged?.(next)
    } catch (cause) {
      if (id === sequence.current) setMessage({ key, text: errorText(cause), error: true })
    } finally {
      setBusy(false)
      setPending(null)
    }
  }

  const pick = (mode: string) => {
    const change = nativePermissionChange(view, mode)
    if (change.kind === 'none') return
    if (change.kind === 'refused') { setMessage({ key, text: change.reason, error: true }); return }
    if (change.kind === 'apply') { void apply(mode); return }
    setPending({ key, mode, change })
  }

  const badge = nativeSwitchBadge(view)
  const shownPending = pending && pending.key === key ? pending : null
  const shownMessage = message && message.key === key ? message : null
  return (
    <span className="fox-agent-backend-native-permission" data-backend={view.catalog.backend} data-current={view.current}>
      <select
        aria-label="权限"
        className="fox-agent-backend-native-select"
        value={view.current}
        disabled={busy}
        title={view.draft ? undefined : view.catalog.switchNote}
        onChange={(event) => pick(event.target.value)}
      >
        {rows.map((row) => (
          <option key={row.value} value={row.value} disabled={row.disabled} title={row.title}>
            {row.label}{row.disabled ? '（不可用）' : ''}
          </option>
        ))}
      </select>
      {badge && <span className="fox-agent-backend-notice" data-native-badge="next_turn">{badge}</span>}
      {/* Every greyed option says why, inline (the full-access lock mid-session too, 13.19 fix ③). */}
      {rows.filter((row) => row.disabled && row.reason).map((row) => (
        <span key={row.value} className="fox-agent-backend-notice" role="note" data-disabled-option={row.value}>{row.label}：{row.reason}</span>
      ))}
      {shownPending?.change.kind === 'confirm_full_access' && (
        <span className="fox-agent-backend-risk" role="alertdialog" aria-label="确认完全访问">
          {shownPending.change.points.join('')}{FULL_ACCESS_PRIVATE_HOME_NOTE}
          <button type="button" onClick={() => void apply(shownPending.mode)}>确认使用完全访问</button>
          <button type="button" onClick={() => setPending(null)}>取消</button>
        </span>
      )}
      {shownPending?.change.kind === 'confirm_revoke' && (
        <span className="fox-agent-backend-risk" role="alertdialog" aria-label="撤销完全访问">
          {shownPending.change.notice}
          <button type="button" onClick={() => void apply(shownPending.mode)}>撤销并重启到沙箱</button>
          <button type="button" onClick={() => setPending(null)}>取消</button>
        </span>
      )}
      {shownMessage && <span className={shownMessage.error ? 'fox-agent-backend-notice fox-agent-backend-refusal' : 'fox-agent-backend-notice'} role={shownMessage.error ? 'alert' : 'status'}>{shownMessage.text}</span>}
    </span>
  )
}
