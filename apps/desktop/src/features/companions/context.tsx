import { createContext, useCallback, useContext, useEffect, useRef, useState } from 'react'
import { notify as toast } from '@/features/notifications'
import { desktopClient, desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import type { ConversationDetail, ProjectRecord } from '@/features/conversations/model/types'
import { contacts, type Companion, type CompanionsList } from './api'

export const showContactError = (error: unknown) => toast.error(desktopErrorDetails(error).message)
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
      if (/^(run\.(started|completed|failed|cancelled|interrupted)|message\.completed)$/.test(event.event.type)) void refresh()
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
  add: () => void
  leave: (contact: Companion) => void
  refreshConversation: () => Promise<void>
  /** C05: jump from the profile's file list back to the source message. */
  locateRequest?: ContactLocateRequest | null
  jumpToMessage?: (bubbleId: string) => void
}

export const ContactsContext = createContext<ContactsContextValue | null>(null)
export const useContacts = () => useContext(ContactsContext)
export function useReadCursor(contact: Companion | undefined, detail: ConversationDetail | null, visible: boolean, refresh: () => Promise<void>) {
  const ordinal = detail?.messages.findLast(message => message.role === 'assistant' && message.status === 'completed')?.ordinal
  const id = contact?.id
  const read = contact?.lastReadOrdinal
  useEffect(() => {
    if (!visible || !id || !ordinal || read === undefined || ordinal <= read) return
    const update = () => {
      if (document.visibilityState === 'visible' && document.hasFocus()) void contacts.markRead(id, ordinal).then(refresh).catch(showContactError)
    }
    update()
    window.addEventListener('focus', update)
    document.addEventListener('visibilitychange', update)
    return () => {
      window.removeEventListener('focus', update)
      document.removeEventListener('visibilitychange', update)
    }
  }, [id, ordinal, read, visible, refresh])
}
