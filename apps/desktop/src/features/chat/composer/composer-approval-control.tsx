// The ONE approval mode selector of the composer (composer batch 2026-10-31),
// left of the send button. It replaces the duplicate picker that used to sit
// on the backend bar above the input.
//
// * The options, their full names, the two-character button names, which
//   option needs the first-time confirmation and when a change applies are
//   the Host's (`fox_agent_backend::native_permission`); this file only shows
//   them. Fox / PiNative: 只读 / 询问 / 自动 / 完全访问. pi: the same four
//   (enforced by the Fox pi-gate). Codex: 请求审批 / 帮我批准 / 完全访问权限.
//   dsh: 仅可查看 / 工作区完全修改 / 完全权限.
// * Greyed options stay in the menu with their reason.
// * A new external conversation asks the Host to carry the previous choice's
//   tier when its Agent is switched (`carryTier`); a fresh one starts at the
//   Agent's default. Full access is never carried: it takes the user's own
//   pick and the first-time confirmation.
// * The button reads what the backend really runs: a choice the live
//   process has not taken yet (dsh restarts at the next turn) shows the small
//   note「下一轮生效」inside the button, never on a row of its own.
import { Check, CircleHelp, Eye, Pencil, Play, ShieldAlert, Sparkles, UserCheck } from 'lucide-react'
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { PromptInputActionMenu, PromptInputActionMenuContent, PromptInputActionMenuItem, PromptInputActionMenuTrigger } from '@/components/ai-elements/prompt-input'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { FULL_ACCESS_PRIVATE_HOME_NOTE } from '@/features/conversations/model/agent-backend-onboarding'
import {
  catalogRows,
  nativeAppliedNotice,
  nativePendingNote,
  nativePermissionChange,
  nativePermissionRows,
  nativeSwitchBadge,
  type FoxPermissionTier,
  type NativePermissionCatalog,
  type NativePermissionChange,
  type NativePermissionRequest,
  type NativePermissionRow,
  type NativePermissionView,
} from '@/features/conversations/model/agent-native-permission'
import { type ComposerAccessMode, FOX_ACCESS_MODES } from '../access-mode'

function tierIcon(tier: FoxPermissionTier | null, size?: number): ReactNode {
  switch (tier) {
    case 'read_only': return <Eye size={size} />
    case 'auto': return <Play size={size} />
    case 'full_access': return <ShieldAlert size={size} />
    default: return <CircleHelp size={size} />
  }
}

function rowIcon(row: NativePermissionRow, backend: string, size?: number): ReactNode {
  if (backend === 'codex' && row.tier === 'ask') return <UserCheck size={size} />
  if (backend === 'codex' && row.tier === 'auto') return <Sparkles size={size} />
  if (backend === 'deepseek' && row.tier === 'ask') return <Pencil size={size} />
  return tierIcon(row.tier, size)
}

/** The menu both controls render: two-character button, full names inside. */
function ApprovalMenu({ backend, rows, current, disabled, tooltip, note, onPick }: {
  backend: string
  rows: NativePermissionRow[]
  current: string
  disabled?: boolean
  tooltip: string
  /** Small note inside the button (「下一轮生效」). */
  note?: string | null
  onPick: (value: string) => void
}) {
  const active = rows.find((row) => row.value === current) ?? null
  return (
    <PromptInputActionMenu>
      <PromptInputActionMenuTrigger className="fox-execution-control" data-backend={backend} data-current={current} data-pending={note ? 'true' : undefined} disabled={disabled} tooltip={tooltip} aria-label={`审批模式：${active?.label ?? current}${note ? `（${note}）` : ''}`}>
        {active ? rowIcon(active, backend, 14) : <CircleHelp size={14} />}
        <span>{active?.shortLabel ?? '审批'}</span>
        {note && <small className="fox-execution-control-note">{note}</small>}
      </PromptInputActionMenuTrigger>
      <PromptInputActionMenuContent className="fox-add-menu fox-execution-menu" data-backend={backend}>
        {rows.map((row) => (
          <PromptInputActionMenuItem key={row.value} data-mode={row.value} disabled={row.disabled} title={row.title} onSelect={() => { if (!row.disabled) onPick(row.value) }}>
            {rowIcon(row, backend)}
            <span className="fox-execution-option">
              <span>{row.label}</span>
              {row.reason && <small className="fox-execution-option-reason" data-disabled-option={row.value}>{row.reason}</small>}
            </span>
            {row.value === current && <Check className="fox-menu-check" />}
          </PromptInputActionMenuItem>
        ))}
      </PromptInputActionMenuContent>
    </PromptInputActionMenu>
  )
}

/**
 * Fox / PiNative conversation: Fox's four modes. The list comes from the
 * Host (`approvalModes`); without one (browser preview, older Host) the
 * built-in table with the same ids is used.
 */
export function FoxApprovalControl({ catalog, value, disabled, onChange }: {
  catalog: NativePermissionCatalog | null
  value: ComposerAccessMode | null | undefined
  disabled?: boolean
  onChange?: (mode: ComposerAccessMode) => void | Promise<void>
}) {
  const current = value ?? 'ask'
  const rows: NativePermissionRow[] = catalog
    ? catalogRows(catalog, current)
    : FOX_ACCESS_MODES.map(([mode, short, label, description]) => ({
      value: mode,
      label,
      shortLabel: short.length > 2 ? Array.from(short).slice(0, 2).join('') : short,
      tier: mode === 'allow' ? 'auto' : mode,
      needsConfirmation: mode === 'full_access',
      title: description,
      disabled: false,
      reason: null,
      fullAccess: mode === 'full_access',
    }))
  return (
    <ApprovalMenu
      backend="fox"
      rows={rows}
      current={current}
      disabled={disabled}
      tooltip={`审批模式（${catalog?.switchNote || '运行中切换，从下一次工具调用起生效'}）`}
      onPick={(mode) => void onChange?.(mode as ComposerAccessMode)}
    />
  )
}

export interface NativeApprovalClient {
  agentBackendNativePermissions: (request: NativePermissionRequest) => Promise<NativePermissionView>
  agentBackendNativePermissionSet: (request: NativePermissionRequest & { mode?: string }) => Promise<NativePermissionView>
}

function errorText(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause)
}

/**
 * External Agent (pi / Codex / dsh): the Agent's own modes from the Host.
 * `conversationId` null = a new conversation of `backend`; `carryTier` is
 * the tier to carry when this draft's Agent was just switched (`''` fresh).
 */
export function NativeApprovalControl({ client, conversationId, backend, projectId = null, carryTier = '', tick = 0, disabled, onView }: {
  client: NativeApprovalClient
  conversationId: string | null
  backend: string
  projectId?: string | null
  carryTier?: FoxPermissionTier | ''
  tick?: number
  disabled?: boolean
  /** Every view the control shows (the caller keeps its tier for the next switch). */
  onView?: (view: NativePermissionView) => void
}) {
  const draft = !conversationId
  const key = JSON.stringify([conversationId, backend, projectId])
  const [state, setState] = useState<{ key: string; view: NativePermissionView } | null>(null)
  const [pending, setPending] = useState<{ key: string; mode: string; change: NativePermissionChange } | null>(null)
  const [message, setMessage] = useState<{ key: string; text: string; error: boolean } | null>(null)
  const [busy, setBusy] = useState(false)
  const sequence = useRef(0)
  const onViewRef = useRef(onView)
  onViewRef.current = onView
  // A draft carries the tier once per Agent switch (not on every refresh).
  const carriedFor = useRef<string | null>(null)
  const carryRef = useRef(carryTier)
  carryRef.current = carryTier

  const request = useCallback((): NativePermissionRequest => (conversationId ? { conversationId } : { backend, projectId }), [conversationId, backend, projectId])

  useEffect(() => {
    const id = ++sequence.current
    setState((current) => (current && current.key === key ? current : null))
    setPending(null)
    setMessage((current) => (current && current.key === key ? current : null))
    const carryKey = draft ? backend : null
    const load = carryKey && carriedFor.current !== carryKey
      ? client.agentBackendNativePermissionSet({ ...request(), carryTier: carryRef.current ?? '' })
      : client.agentBackendNativePermissions(request())
    carriedFor.current = carryKey
    load
      .then((view) => {
        if (id !== sequence.current) return
        setState({ key, view })
        onViewRef.current?.(view)
      })
      .catch(() => { if (id === sequence.current) setState(null) })
  }, [client, key, tick, request, draft, backend])

  const view = state && state.key === key ? state.view : null
  const rows = nativePermissionRows(view)
  if (!view || rows.length === 0) return null

  const apply = async (mode: string, confirmFullAccess = false) => {
    setBusy(true)
    const id = sequence.current
    try {
      const next = await client.agentBackendNativePermissionSet({ ...request(), mode, ...(confirmFullAccess ? { confirmFullAccess: true } : {}) })
      if (id !== sequence.current) return
      setState({ key, view: next })
      onViewRef.current?.(next)
      const notice = nativeAppliedNotice(next)
      setMessage(notice ? { key, text: notice, error: next.applied === 'failed' } : null)
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

  const shownPending = pending && pending.key === key ? pending : null
  const shownMessage = message && message.key === key ? message : null
  const note = nativePendingNote(view)
  const timing = nativeSwitchBadge(view) ?? view.catalog.switchNote
  const fullRow = shownPending ? rows.find((row) => row.value === shownPending.mode) : null
  return (
    <>
      <ApprovalMenu
        backend={view.catalog.backend}
        rows={rows}
        current={view.current}
        disabled={disabled || busy}
        tooltip={`审批模式（${timing}）${shownMessage ? `：${shownMessage.text}` : ''}`}
        note={note}
        onPick={pick}
      />
      {shownMessage?.error && <span className="fox-execution-control-error" role="alert">{shownMessage.text}</span>}
      <Dialog open={shownPending?.change.kind === 'confirm_full_access'} onOpenChange={(open) => { if (!open) setPending(null) }}>
        <DialogContent aria-label="确认完全访问">
          <DialogHeader>
            <DialogTitle>确认使用「{fullRow?.label ?? '完全访问'}」？</DialogTitle>
            <DialogDescription>
              {shownPending?.change.kind === 'confirm_full_access' ? shownPending.change.points.join('') : ''}{FULL_ACCESS_PRIVATE_HOME_NOTE}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setPending(null)}>取消</Button>
            <Button type="button" onClick={() => { if (shownPending) void apply(shownPending.mode, true) }}>确认使用{fullRow?.label ?? '完全访问'}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog open={shownPending?.change.kind === 'confirm_revoke'} onOpenChange={(open) => { if (!open) setPending(null) }}>
        <DialogContent aria-label="撤销完全访问">
          <DialogHeader>
            <DialogTitle>切回受限权限？</DialogTitle>
            <DialogDescription>{shownPending?.change.kind === 'confirm_revoke' ? shownPending.change.notice : ''}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setPending(null)}>取消</Button>
            <Button type="button" onClick={() => { if (shownPending) void apply(shownPending.mode) }}>切回受限权限</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
