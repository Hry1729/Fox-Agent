/**
 * One owner for the conversation's mid-run supplementary requests
 * ("运行中补充要求").
 *
 * The main composer and the status list above it are two views over this single
 * hook, so there is exactly one editor, one polling loop and one durable source
 * of truth. The Host owns every state transition; this hook only reads the
 * listing and submits the two lanes.
 *
 * Binding rules this hook enforces:
 *
 * - Every request is bound to the conversation it was typed in. A listing that
 *   resolves after the user switched conversations is discarded instead of
 *   overwriting the new conversation's rows.
 * - Enqueueing resolves the Run on the Host from the conversation id only; the
 *   client never supplies a run id, so a stale view cannot steer the wrong task.
 * - Repeated submits of one draft reuse one client message id, so a double click
 *   or a retry after a dropped connection is an idempotent no-op on the Host
 *   rather than a second delivery.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  desktopClient,
  desktopErrorDetails,
  desktopRuntimeAvailable,
  type RunSteeringLane,
  type RunSteeringRecord,
} from '@/features/conversations/api/desktop-client'
import { steeringStripView, type SteeringStripView } from '../components/steering-presentation'

export interface RunSteeringState {
  rows: RunSteeringRecord[]
  runId: string | null
  runState: string | null
  acceptsSteering: boolean
  /** Durable, lane-aware presentation of the rows above. */
  view: SteeringStripView
  busy: boolean
  error: string | null
  /** Submit one request on an explicit lane. Resolves to whether it was stored. */
  submit: (content: string, lane: RunSteeringLane) => Promise<boolean>
  /** Remove a still-held `next-turn` request. */
  discard: (messageId: string) => Promise<boolean>
  reload: (silent?: boolean) => Promise<void>
}

function newMessageId() {
  const random = globalThis.crypto?.randomUUID?.()
  return random ?? `steering-${Date.now()}-${Math.random().toString(16).slice(2)}`
}

/**
 * The identity of a draft whose submission has not been confirmed yet.
 *
 * It is persisted per conversation so a webview reload or an application
 * restart between the Host storing the row and this client learning about it
 * re-submits the SAME identity — which the Host treats as a duplicate receipt
 * rather than a second delivery. In-memory only, this window was exactly the
 * "断线/重启后重复执行" gap.
 */
const PENDING_KEY_PREFIX = 'fox.steering.pending.'

interface PendingSubmit {
  conversationId: string
  content: string
  messageId: string
}

function readPendingSubmit(conversationId: string): PendingSubmit | null {
  try {
    const raw = globalThis.localStorage?.getItem(`${PENDING_KEY_PREFIX}${conversationId}`)
    if (!raw) return null
    const parsed = JSON.parse(raw) as Partial<PendingSubmit>
    if (typeof parsed?.content !== 'string' || typeof parsed?.messageId !== 'string') return null
    return { conversationId, content: parsed.content, messageId: parsed.messageId }
  } catch {
    // A malformed or unavailable store only costs the dedup window; it must
    // never block submitting.
    return null
  }
}

function writePendingSubmit(conversationId: string, pending: { content: string; messageId: string } | null) {
  try {
    if (pending === null) globalThis.localStorage?.removeItem(`${PENDING_KEY_PREFIX}${conversationId}`)
    else globalThis.localStorage?.setItem(`${PENDING_KEY_PREFIX}${conversationId}`, JSON.stringify(pending))
  } catch {
    // Storage is best-effort; the in-memory ref still covers a live session.
  }
}

export function useRunSteering({
  conversationId,
  active,
  enabled = true,
}: {
  conversationId?: string
  active: boolean
  enabled?: boolean
}): RunSteeringState {
  const [rows, setRows] = useState<RunSteeringRecord[]>([])
  const [runId, setRunId] = useState<string | null>(null)
  const [runState, setRunState] = useState<string | null>(null)
  const [acceptsSteering, setAcceptsSteering] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // A pending submit keeps one identity across retries so the Host can dedup it,
  // and that identity survives a reload through `localStorage`.
  const pendingIdRef = useRef<{ conversationId: string; content: string; messageId: string } | null>(null)
  const conversationIdRef = useRef<string | undefined>(conversationId)
  conversationIdRef.current = conversationId
  const enabledRef = useRef(enabled)
  enabledRef.current = enabled

  const load = useCallback(async (silent = false) => {
    const id = conversationIdRef.current
    if (!desktopRuntimeAvailable || !enabledRef.current || !id) {
      setRows([])
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
      // every request instead of an empty list.
      const response = await desktopClient.listRunSteering(id)
      // The conversation may have changed while this was in flight; a listing
      // for another conversation must never replace this one's rows.
      if (conversationIdRef.current !== id) return
      setRows(response.messages)
      setRunId(response.runId)
      setRunState(response.runState)
      setAcceptsSteering(response.acceptsSteering)
      setError(null)
    } catch (cause) {
      if (!silent && conversationIdRef.current === id) setError(desktopErrorDetails(cause).message)
    }
  }, [])

  // Load on conversation switch / Run activity changes; poll while active and
  // reload on every kernel invalidation (dispatch settle flips the statuses).
  useEffect(() => {
    setRows([])
    setRunId(null)
    setRunState(null)
    setAcceptsSteering(false)
    setError(null)
    pendingIdRef.current = null
    void load()
  }, [conversationId, enabled, load])

  useEffect(() => {
    if (!active || !enabled) {
      void load(true)
      return
    }
    void load(true)
    const timer = window.setInterval(() => void load(true), 1500)
    let unlisten: (() => void) | undefined
    let disposed = false
    void desktopClient.listenKernelStateInvalidations(() => void load(true)).then((fn) => {
      if (disposed) fn()
      else unlisten = fn
    })
    return () => {
      disposed = true
      window.clearInterval(timer)
      unlisten?.()
    }
  }, [active, conversationId, enabled, load])

  const submit = useCallback(async (content: string, lane: RunSteeringLane) => {
    const id = conversationIdRef.current
    const text = content.trim()
    if (!id || !text || !enabledRef.current) return false
    // The same draft keeps the same identity until the Host confirms it was
    // stored, so a double click, a retry after a dropped connection, a webview
    // reload or an application restart are all idempotent receipts rather than a
    // second delivery.
    const pending = pendingIdRef.current ?? readPendingSubmit(id)
    const messageId = pending && pending.conversationId === id && pending.content === text
      ? pending.messageId
      : newMessageId()
    const record = { conversationId: id, content: text, messageId }
    pendingIdRef.current = record
    writePendingSubmit(id, { content: text, messageId })
    setBusy(true)
    setError(null)
    try {
      await desktopClient.enqueueRunSteering(id, text, { messageId, lane })
      pendingIdRef.current = null
      writePendingSubmit(id, null)
      if (conversationIdRef.current === id) await load(true)
      return true
    } catch (cause) {
      const details = desktopErrorDetails(cause)
      // The Run ended between the UI state and the command. Reconcile first,
      // then report: the reconciliation clears the listing error, and the reason
      // this submit failed must stay visible so the user knows the text was not
      // accepted (and still holds it in the draft).
      if (details.code === 'steering.no_active_run') await load(true)
      if (conversationIdRef.current === id) setError(details.message)
      return false
    } finally {
      setBusy(false)
    }
  }, [load])

  const discard = useCallback(async (messageId: string) => {
    const id = conversationIdRef.current
    if (!id || !enabledRef.current) return false
    setBusy(true)
    setError(null)
    try {
      await desktopClient.discardRunSteering(id, messageId)
      if (conversationIdRef.current === id) await load(true)
      return true
    } catch (cause) {
      const details = desktopErrorDetails(cause)
      if (conversationIdRef.current === id) setError(details.message)
      return false
    } finally {
      setBusy(false)
    }
  }, [load])

  const view = useMemo(
    () =>
      steeringStripView({
        active,
        runId,
        runState,
        acceptsSteering,
        rows: rows.map((row) => ({
          messageId: row.messageId,
          content: row.content,
          status: row.status,
          lane: row.lane,
        })),
      }),
    [acceptsSteering, active, rows, runId, runState],
  )

  return { rows, runId, runState, acceptsSteering, view, busy, error, submit, discard, reload: load }
}
