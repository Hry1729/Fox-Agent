import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { KnowledgeBaseRecord, KnowledgeDetailRecord } from '@/features/conversations/model/types'

export function useKnowledgeBases(enabled = true) {
  const [items, setItems] = useState<KnowledgeBaseRecord[]>([])
  const [loading, setLoading] = useState(desktopRuntimeAvailable)
  const [error, setError] = useState<string | null>(null)
  const refresh = useCallback(async () => {
    if (!desktopRuntimeAvailable || !enabled) {
      setItems([])
      setLoading(false)
      return
    }
    setLoading(true)
    try { setItems(await desktopClient.listKnowledgeBases()); setError(null) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLoading(false) }
  }, [enabled])
  useEffect(() => { void refresh() }, [refresh])
  return useMemo(() => ({ items, loading, error, refresh }), [error, items, loading, refresh])
}

export function useKnowledgeDetail(id: string | null) {
  const [detail, setDetail] = useState<KnowledgeDetailRecord | null>(null)
  const [document, setDocument] = useState<unknown>(null)
  const [documentId, setDocumentId] = useState<string | null>(null)
  const [documentLoading, setDocumentLoading] = useState(false)
  const documentRequest = useRef(0)
  const [graph, setGraph] = useState<unknown>(null)
  const [graphLabels, setGraphLabels] = useState<unknown>(null)
  const [graphStats, setGraphStats] = useState<unknown>(null)
  const [graphLoading, setGraphLoading] = useState(false)
  const [downloadingId, setDownloadingId] = useState<string | null>(null)
  const [downloadProgress, setDownloadProgress] = useState<{ bytesWritten: number; totalBytes: number | null } | null>(null)
  const activeDownloadId = useRef<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const refresh = useCallback(async () => {
    if (!id || !desktopRuntimeAvailable) return
    setLoading(true)
    try { setDetail(await desktopClient.getKnowledgeDetail(id)); setError(null) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLoading(false) }
  }, [id])
  useEffect(() => { void refresh() }, [refresh])
  const openDocument = useCallback(async (documentId: string) => {
    if (!id) return
    const request = ++documentRequest.current
    setDocumentId(documentId)
    setDocument(null)
    setDocumentLoading(true)
    try {
      const value = await desktopClient.getKnowledgeDocument(id, documentId)
      if (documentRequest.current !== request) return
      setDocument(value)
      setError(null)
    } catch (cause) {
      if (documentRequest.current !== request) return
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      if (documentRequest.current === request) setDocumentLoading(false)
    }
  }, [id])
  const downloadDocument = useCallback(async (documentId: string, filename: string) => {
    if (!id || downloadingId) return null
    const downloadId = crypto.randomUUID()
    activeDownloadId.current = downloadId
    setDownloadingId(documentId)
    setDownloadProgress({ bytesWritten: 0, totalBytes: null })
    let unlisten: (() => void) | undefined
    try {
      unlisten = await desktopClient.listenKnowledgeDownloadProgress((progress) => {
        if (progress.downloadId !== downloadId) return
        setDownloadProgress({ bytesWritten: progress.bytesWritten, totalBytes: progress.totalBytes })
      })
      const result = await desktopClient.downloadKnowledgeDocument(downloadId, id, documentId, filename)
      setError(null)
      return result
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      return null
    } finally {
      unlisten?.()
      activeDownloadId.current = null
      setDownloadingId(null)
      setDownloadProgress(null)
    }
  }, [downloadingId, id])
  const cancelDownload = useCallback(async () => {
    if (!activeDownloadId.current) return false
    try {
      const cancelled = await desktopClient.cancelKnowledgeDocumentDownload(activeDownloadId.current)
      setError(null)
      return cancelled
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      return false
    }
  }, [])
  const loadGraph = useCallback(async (keyword = '', maxDepth = 2, maxNodes = 100) => {
    if (!id) return
    setGraphLoading(true)
    try { setGraph(await desktopClient.getKnowledgeGraph(id, keyword, maxDepth, maxNodes)); setError(null) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setGraphLoading(false) }
  }, [id])
  const loadGraphMetadata = useCallback(async () => {
    if (!id) return
    try {
      const [labels, stats] = await Promise.all([
        desktopClient.getKnowledgeGraphLabels(id),
        desktopClient.getKnowledgeGraphStats(id),
      ])
      setGraphLabels(labels)
      setGraphStats(stats)
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }, [id])
  return useMemo(() => ({ detail, document, documentId, documentLoading, graph, graphLabels, graphStats, graphLoading, downloadingId, downloadProgress, loading, error, refresh, openDocument, downloadDocument, cancelDownload, loadGraph, loadGraphMetadata }), [detail, document, documentId, documentLoading, downloadingId, downloadProgress, error, graph, graphLabels, graphLoading, graphStats, loadGraph, loadGraphMetadata, loading, openDocument, downloadDocument, cancelDownload, refresh])
}
