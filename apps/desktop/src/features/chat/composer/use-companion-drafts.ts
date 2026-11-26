import { useCallback, useMemo, useRef, useState, type SetStateAction } from 'react'
import type { ComposerInitialAttachments } from './composer-initial-attachments'

export function useCompanionDrafts(conversationId: string | null, visible: boolean) {
  const [textByConversation, setTextByConversation] = useState<Record<string, string>>({})
  const filesByConversation = useRef(new Map<string, File[]>())

  const setText = useCallback((next: SetStateAction<string>) => {
    if (!conversationId) return
    setTextByConversation(current => {
      const previous = current[conversationId] ?? ''
      const text = typeof next === 'function' ? next(previous) : next
      return text === previous ? current : { ...current, [conversationId]: text }
    })
  }, [conversationId])

  const setFiles = useCallback((files: File[]) => {
    if (!conversationId) return
    if (files.length) filesByConversation.current.set(conversationId, files)
    else filesByConversation.current.delete(conversationId)
  }, [conversationId])

  const restoredAttachments = useMemo<ComposerInitialAttachments | null>(() => {
    const files = visible && conversationId ? filesByConversation.current.get(conversationId) : undefined
    return files?.length ? { requestId: -1, files, restoreDraft: true } : null
  }, [conversationId, visible])

  return { text: conversationId ? textByConversation[conversationId] ?? '' : '', setText, setFiles, restoredAttachments }
}
