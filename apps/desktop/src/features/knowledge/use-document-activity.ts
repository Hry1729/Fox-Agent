import { useCallback, useEffect, useRef, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type {
  KnowledgeDocumentActivity,
  KnowledgeDocumentAnnotation,
  KnowledgeDocumentBookmark,
  KnowledgeDocumentReadingState,
} from '@/features/conversations/model/types'
import { documentActivityOrder, type DocumentViewPosition } from './document-activity-model'

const EMPTY_ACTIVITY: KnowledgeDocumentActivity = {
  readingState: null,
  bookmarks: [],
  annotations: [],
}

export function useDocumentActivity(
  knowledgeBaseId: string,
  documentId: string,
  sourceRevision: string | null,
) {
  const [activity, setActivity] = useState<KnowledgeDocumentActivity>(EMPTY_ACTIVITY)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const saveTimer = useRef<number | null>(null)
  const pendingPosition = useRef<DocumentViewPosition | null>(null)

  const refresh = useCallback(async () => {
    if (!desktopRuntimeAvailable || !knowledgeBaseId || !documentId) {
      setActivity(EMPTY_ACTIVITY)
      setLoading(false)
      return
    }
    setLoading(true)
    try {
      setActivity(await desktopClient.getKnowledgeDocumentActivity(knowledgeBaseId, documentId))
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }, [documentId, knowledgeBaseId])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const persistPosition = useCallback(async (position: DocumentViewPosition) => {
    try {
      const readingState = await desktopClient.saveKnowledgeDocumentReadingState({
        knowledgeBaseId,
        documentId,
        sourceRevision,
        page: Math.max(1, Math.round(position.page)),
        scrollOffset: Math.max(0, position.scrollOffset ?? 0),
        zoom: position.zoom ?? null,
      })
      setActivity((current) => ({ ...current, readingState }))
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }, [documentId, knowledgeBaseId, sourceRevision])

  const savePosition = useCallback((position: DocumentViewPosition) => {
    pendingPosition.current = position
    if (saveTimer.current != null) window.clearTimeout(saveTimer.current)
    saveTimer.current = window.setTimeout(() => {
      saveTimer.current = null
      const pending = pendingPosition.current
      pendingPosition.current = null
      if (pending) void persistPosition(pending)
    }, 500)
  }, [persistPosition])

  useEffect(() => () => {
    if (saveTimer.current != null) window.clearTimeout(saveTimer.current)
    const pending = pendingPosition.current
    pendingPosition.current = null
    if (pending) void persistPosition(pending)
  }, [persistPosition])

  const addBookmark = useCallback(async (position: DocumentViewPosition, label = '') => {
    const bookmark = await desktopClient.saveKnowledgeDocumentBookmark({
      knowledgeBaseId,
      documentId,
      sourceRevision,
      page: Math.max(1, Math.round(position.page)),
      label,
    })
    setActivity((current) => ({
      ...current,
      bookmarks: [...current.bookmarks, bookmark].sort(documentActivityOrder),
    }))
    return bookmark
  }, [documentId, knowledgeBaseId, sourceRevision])

  const removeBookmark = useCallback(async (bookmark: KnowledgeDocumentBookmark) => {
    await desktopClient.deleteKnowledgeDocumentBookmark(knowledgeBaseId, documentId, bookmark.id)
    setActivity((current) => ({
      ...current,
      bookmarks: current.bookmarks.filter((item) => item.id !== bookmark.id),
    }))
  }, [documentId, knowledgeBaseId])

  const addAnnotation = useCallback(async (
    position: DocumentViewPosition,
    note: string,
    color: KnowledgeDocumentAnnotation['color'] = 'blue',
  ) => {
    const annotation = await desktopClient.saveKnowledgeDocumentAnnotation({
      knowledgeBaseId,
      documentId,
      sourceRevision,
      annotationType: 'note',
      page: Math.max(1, Math.round(position.page)),
      note,
      color,
    })
    setActivity((current) => ({
      ...current,
      annotations: [...current.annotations, annotation].sort(documentActivityOrder),
    }))
    return annotation
  }, [documentId, knowledgeBaseId, sourceRevision])

  const removeAnnotation = useCallback(async (annotation: KnowledgeDocumentAnnotation) => {
    await desktopClient.deleteKnowledgeDocumentAnnotation(knowledgeBaseId, documentId, annotation.id)
    setActivity((current) => ({
      ...current,
      annotations: current.annotations.filter((item) => item.id !== annotation.id),
    }))
  }, [documentId, knowledgeBaseId])

  return {
    activity,
    loading,
    error,
    refresh,
    savePosition,
    addBookmark,
    removeBookmark,
    addAnnotation,
    removeAnnotation,
  }
}

export type { DocumentViewPosition } from './document-activity-model'
