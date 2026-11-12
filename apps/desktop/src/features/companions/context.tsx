import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import { notify as toast } from '@/features/notifications'
import { desktopClient, desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import type { ContactBubbleMarkRecord, ConversationDetail, ProjectRecord } from '@/features/conversations/model/types'
import { contacts, type Companion, type CompanionsList, type QuotePreview } from './api'
import { companionBubbleOrders, readWalkBase, reportableReadOrder, shouldReportRead } from './read-position'

export const showContactError = (error: unknown) => toast.error(desktopErrorDetails(error).message)
export const shouldRefreshContacts = (notice: { event: { type?: string; tool?: string; isError?: boolean } }) =>
  /^(run\.(started|completed|failed|cancelled|interrupted)|message\.completed)$/.test(notice.event.type ?? '')
  || (notice.event.type === 'tool.completed' && notice.event.tool === 'companion_profile_update' && notice.event.isError !== true)

export function useContactList(enabled: boolean) {
  const [list, setList] = useState<CompanionsList>({ companions: [], pendingDeletions: [] })
  const [error, setError] = useState('')
  const version = useRef(0)
  const refresh = useCallback(async () => {
    if (!enabled) return
    const ticket = ++version.current
    try {
      const next = await contacts.list()
      if (ticket === version.current) { setList(next); setError('') }
    } catch (cause) {
      if (ticket === version.current) setError(desktopErrorDetails(cause).message)
    }
  }, [enabled])
  useEffect(() => {
    if (!enabled) return
    void refresh()
    const listener = desktopClient.listenRuntimeEvents(event => {
      if (shouldRefreshContacts(event)) void refresh()
    })
    return () => { ++version.current; void listener.then(stop => stop()).catch(() => { }) }
  }, [enabled, refresh])
  return { list, refresh, error }
}

export interface ContactLocateRequest {
  /** K1 stable bubble id (a user bubble's id is its message id). */
  id: string
  /** Changes on every request so repeats retrigger the locate effect. */
  key: number
}

export interface ContactsContextValue extends ReturnType<typeof useContactList> {
  current: Companion | undefined
  privateChat: boolean
  projects: ProjectRecord[]
  model: string
  openSharedMemory: () => void
  askToOrganize: () => void
  permission: string
  setPermission: (mode: ProjectRecord['permissionMode']) => void
  open: (contact: Companion, profile?: boolean) => void
  showProfile: () => void
  leave: (contact: Companion) => void
  refreshConversation: () => Promise<void>
  /** C05: jump from the profile's file list back to the source message. */
  locateRequest?: ContactLocateRequest | null
  jumpToMessage?: (bubbleId: string) => void
  /** C07: the composer's quote draft of this private chat, resolved from a
   * committed bubble only. Cleared on send and on switching conversations. */
  quoteDraft?: QuotePreview | null
  quoteBubble?: (bubbleId: string) => Promise<void>
  clearQuote?: () => void
  /** C07: the latest Host-confirmed mark per bubble, shared by every view of
   * this chat so a cancel on the favorites page reaches the bubble menu too. */
  bubbleMarkUpdates?: ReadonlyMap<string, ContactBubbleMarkRecord>
  publishBubbleMark?: (mark: ContactBubbleMarkRecord) => void
}

export const ContactsContext = createContext<ContactsContextValue | null>(null)
export const useContacts = () => useContext(ContactsContext)
/** K6: auto-read follows the bubbles the user actually sees, not window focus or
 * the last loaded message. A bubble counts as seen only when it enters the
 * visible reading area while the page is active; the reported position is a
 * contiguous seen-walk that never jumps an unseen unread interval. A manual
 * unread override resets what counts as seen, so the reading state at the moment
 * of marking cannot immediately clear it — only re-opening and re-reading does. */
export function useReadCursor(contact: Companion | undefined, detail: ConversationDetail | null, visible: boolean, refresh: () => Promise<void>) {
  const id = contact?.id
  const readOrder = contact?.readDeliveryOrder ?? 0
  const overrideFrom = contact?.manualUnreadFromOrder ?? null
  const overrideKey = overrideFrom == null ? null : `${overrideFrom}:${contact?.manualUnreadAt ?? 0}`
  const orders = useMemo(() => companionBubbleOrders(detail?.bubbles), [detail])
  const hasEarlierMessages = detail?.hasEarlierMessages ?? false
  const seenRef = useRef<Set<number>>(new Set())
  const visibleRef = useRef<Set<number>>(new Set())
  const pendingFocusRef = useRef<Set<number>>(new Set())
  const reportedRef = useRef(-1)
  const lastIdRef = useRef<string | undefined>(undefined)
  const wasVisibleRef = useRef(false)
  const lastOverrideRef = useRef<string | null>(overrideKey)
  const stateRef = useRef({ id, readOrder, overrideFrom, orders, visible, hasEarlierMessages })
  stateRef.current = { id, readOrder, overrideFrom, orders, visible, hasEarlierMessages }

  const report = useCallback(() => {
    const state = stateRef.current
    if (!state.visible || !state.id) return
    if (document.visibilityState !== 'visible' || !document.hasFocus()) return
    const base = readWalkBase(state.readOrder, state.overrideFrom)
    const target = reportableReadOrder(state.orders, base, seenRef.current, state.hasEarlierMessages, state.overrideFrom)
    if (!shouldReportRead(target, state.readOrder, state.overrideFrom, reportedRef.current)) return
    reportedRef.current = target
    void contacts.markRead(state.id, target, Date.now()).then(refresh).catch(showContactError)
  }, [refresh])

  // A fresh mark-unread invalidates the seen set without re-observing: bubbles
  // already on screen stay "visible" so they must leave and re-enter (or the chat
  // must be reopened) before they count as read again.
  useEffect(() => {
    if (lastOverrideRef.current === overrideKey) return
    lastOverrideRef.current = overrideKey
    seenRef.current.clear()
    pendingFocusRef.current.clear()
    reportedRef.current = -1
  }, [overrideKey])

  useEffect(() => {
    if (!visible || !id) {
      wasVisibleRef.current = false
      pendingFocusRef.current.clear()
      return
    }
    if (lastIdRef.current !== id) {
      lastIdRef.current = id
      seenRef.current.clear()
      visibleRef.current.clear()
      pendingFocusRef.current.clear()
      reportedRef.current = -1
    } else if (!wasVisibleRef.current) {
      // Reopening the same chat is a fresh viewport entry: bubbles still on
      // screen must re-enter the visible set so re-reading them counts again
      // (this is how a manual unread clears after reopen). The seen set stays,
      // so within one visit marking unread cannot immediately re-clear itself.
      visibleRef.current.clear()
    }
    wasVisibleRef.current = true
    if (typeof IntersectionObserver === 'undefined') return
    const sections = Array.from(document.querySelectorAll<HTMLElement>('[data-delivery-order]'))
    if (!sections.length) return
    const observer = new IntersectionObserver(entries => {
      for (const entry of entries) {
        const order = Number((entry.target as HTMLElement).dataset.deliveryOrder)
        if (Number.isNaN(order)) continue
        if (entry.isIntersecting) {
          if (!visibleRef.current.has(order)) {
            visibleRef.current.add(order)
            if (document.visibilityState === 'visible' && document.hasFocus()) seenRef.current.add(order)
            else pendingFocusRef.current.add(order)
          }
        } else {
          visibleRef.current.delete(order)
          pendingFocusRef.current.delete(order)
        }
      }
      report()
    }, { threshold: 0 })
    sections.forEach(section => observer.observe(section))
    // Bubbles observed while unfocused count only when the window gains focus.
    const onFocus = () => {
      if (document.visibilityState === 'visible' && document.hasFocus()) {
        pendingFocusRef.current.forEach(order => seenRef.current.add(order))
        pendingFocusRef.current.clear()
      }
      report()
    }
    window.addEventListener('focus', onFocus)
    document.addEventListener('visibilitychange', onFocus)
    return () => {
      observer.disconnect()
      window.removeEventListener('focus', onFocus)
      document.removeEventListener('visibilitychange', onFocus)
    }
  }, [id, visible, orders, report])
}
