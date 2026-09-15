// Mid-run supplementary requests ("运行中补充要求") for the conversation's
// active authoritative Run. The text is durably queued by the Host, spliced
// into model input only at the next dispatch boundary, grants no tools and
// never rewrites a frozen request. This strip is a thin view over the two
// read/enqueue Tauri commands; the Host owns every state transition:
// received → delivered → applied (cancelled terminal when the Run ends first).
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ChevronDown, CornerDownLeft, LoaderCircle, MessageSquarePlus } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { notify as toast } from '@/features/notifications'
import {
  desktopClient,
  desktopRuntimeAvailable,
  desktopErrorDetails,
  type RunSteeringRecord,
} from '@/features/conversations/api/desktop-client'
import {
  steeringStatusCopy as statusCopy,
  steeringStripView,
} from './steering-presentation'

// Mirror of the Host bounds (database/repositories/run_steering.rs); checked
// client-side only for immediate feedback — the Host remains authoritative.
const MAX_CONTENT_CHARS = 8_000
const MAX_PENDING_UTF8_BYTES = 64 * 1024

function utf8Bytes(value: string) {
  return new TextEncoder().encode(value).length
}

export function RunSteeringStrip({ conversationId, active }: { conversationId?: string; active: boolean }) {
  const [records, setRecords] = useState<RunSteeringRecord[]>([])
  const [runId, setRunId] = useState<string | null>(null)
  const [runState, setRunState] = useState<string | null>(null)
  const [acceptsSteering, setAcceptsSteering] = useState(false)
  const [draft, setDraft] = useState('')
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [unread, setUnread] = useState(0)
  const conversationIdRef = useRef<string | undefined>(conversationId)
  conversationIdRef.current = conversationId

  const load = useCallback(async (silent = false) => {
    const id = conversationIdRef.current
    if (!desktopRuntimeAvailable || !id) {
      setRecords([])
      setRunId(null)
      setRunState(null)
      setAcceptsSteering(false)
      return
    }
    try {
      // The Host decides which Run this listing describes (the active one when
      // there is one, otherwise the most recent terminal Run) and whether it can
      // still accept input. The UI never infers either from local state, so a
      // finished task or a reopened application still shows what happened to
      // every request instead of an empty strip.
      const response = await desktopClient.listRunSteering(id)
      setRecords(response.messages)
      setRunId(response.runId)
      setRunState(response.runState)
      setAcceptsSteering(response.acceptsSteering)
      setError(null)
    } catch (cause) {
      if (!silent) setError(desktopErrorDetails(cause).message)
    }
  }, [])

  // Load on conversation switch / Run activity changes; poll while active and
  // reload on every kernel invalidation (dispatch settle flips the statuses).
  useEffect(() => {
    setRecords([])
    setRunId(null)
    setRunState(null)
    setAcceptsSteering(false)
    setDraft('')
    setError(null)
    setUnread(0)
    void load()
  }, [conversationId, load])

  useEffect(() => {
    if (!active) {
      void load(true)
      return
    }
    void load(true)
    const timer = window.setInterval(() => void load(true), 1500)
    let unlisten: (() => void) | undefined
    void desktopClient.listenKernelStateInvalidations(() => void load(true)).then((fn) => {
      unlisten = fn
    })
    return () => {
      window.clearInterval(timer)
      unlisten?.()
    }
  }, [active, conversationId, load])

  const outstandingBytes = useMemo(
    () => records
      .filter((row) => row.status === 'received' || row.status === 'delivered')
      .reduce((total, row) => total + utf8Bytes(row.content), 0),
    [records],
  )
  const strip = useMemo(
    () =>
      steeringStripView({
        active,
        runId,
        runState,
        acceptsSteering,
        rows: records.map((row) => ({
          messageId: row.messageId,
          content: row.content,
          status: row.status,
        })),
      }),
    [acceptsSteering, active, records, runId, runState],
  )
  const pendingCount = strip.pending

  // Surface a badge when a queued item changes state while the strip is shut.
  useEffect(() => {
    if (!open && pendingCount > 0) setUnread(pendingCount)
    if (pendingCount === 0) setUnread(0)
  }, [pendingCount, open])

  const enqueue = useCallback(async () => {
    const id = conversationIdRef.current
    const content = draft.trim()
    if (!id || !content || busy) return
    if ([...content].length > MAX_CONTENT_CHARS) {
      setError(`单条补充要求不能超过 ${MAX_CONTENT_CHARS} 字`)
      return
    }
    if (outstandingBytes + utf8Bytes(content) > MAX_PENDING_UTF8_BYTES) {
      setError('排队中的补充要求已达 64 KB 上限，请等待前面的要求被应用后再试')
      return
    }
    setBusy(true)
    setError(null)
    try {
      await desktopClient.enqueueRunSteering(id, content)
      setDraft('')
      await load(true)
      toast.success('补充要求已接收，将在下一个安全边界交给模型')
    } catch (cause) {
      const details = desktopErrorDetails(cause)
      setError(details.message)
      if (details.code === 'steering.no_active_run') {
        // The Run ended between the UI state and the command; reconcile now.
        void load(true)
      }
    } finally {
      setBusy(false)
    }
  }, [busy, draft, load, outstandingBytes])

  if (!desktopRuntimeAvailable || !strip.visible) return null

  // The editor is offered only while the Host says this Run still accepts input.
  // A terminal Run keeps its records visible for auditing but cannot be steered:
  // new requirements belong to the next task, not to a finished one.
  const editable = strip.editable
  const historyOnly = strip.historyOnly

  return (
    <div className="fox-steering-strip">
      <button
        type="button"
        className="fox-steering-header"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <MessageSquarePlus size={13} />
        <span>{strip.label}</span>
        {pendingCount > 0 && <span className="fox-steering-count">{unread || pendingCount}</span>}
        <ChevronDown size={13} className={`fox-steering-chevron ${open ? 'is-open' : ''}`} />
      </button>
      {open && (
        <div className="fox-steering-body">
          <p className="fox-steering-hint">{strip.hint}</p>
          {records.length > 0 && (
            <ul className="fox-steering-list">
              {records.map((row) => {
                const copy = statusCopy(row.status)
                return (
                  <li key={row.messageId} className={`fox-steering-item is-${row.status}`}>
                    <span className="fox-steering-text">{row.content}</span>
                    <span className="fox-steering-status" title={copy.title}>{copy.label}</span>
                  </li>
                )
              })}
            </ul>
          )}
          {editable && (
            <div className="fox-steering-editor">
              <textarea
                value={draft}
                maxLength={MAX_CONTENT_CHARS}
                rows={2}
                placeholder="补充你的要求，例如：另外检查 Sheet2 的合计行"
                onChange={(event) => setDraft(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
                    event.preventDefault()
                    void enqueue()
                  }
                }}
              />
              <div className="fox-steering-actions">
                <small className="fox-steering-budget" title="排队中内容的 UTF-8 字节数">
                  {Math.round(outstandingBytes / 1024)} / 64 KB
                </small>
                <Button
                  type="button"
                  size="sm"
                  disabled={busy || !draft.trim() || outstandingBytes >= MAX_PENDING_UTF8_BYTES}
                  onClick={() => void enqueue()}
                >
                  {busy ? <LoaderCircle className="animate-spin" size={13} /> : <CornerDownLeft size={13} />}
                  投递要求
                </Button>
              </div>
            </div>
          )}
          {error && <p className="fox-steering-error">{error}</p>}
        </div>
      )}
    </div>
  )
}
