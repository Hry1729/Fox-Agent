import { useEffect, useRef, useState } from 'react'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { ProjectFileEntry } from '@/features/conversations/model/types'
import { userFacingCause } from '../send-failure-notice'

export function useProjectFiles(conversationId: string | null, enabled: boolean, refreshKey: string, query: string, projectRoot = '') {
  const [directories, setDirectories] = useState<string[]>([''])
  const [entries, setEntries] = useState<ProjectFileEntry[]>([])
  const [results, setResults] = useState<ProjectFileEntry[]>([])
  const [truncated, setTruncated] = useState(false)
  const [loading, setLoading] = useState(false)
  const [searchLoading, setSearchLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const cache = useRef(new Map<string, ProjectFileEntry[]>())
  const lastRefresh = useRef('')
  const directoryKey = JSON.stringify(directories)

  useEffect(() => {
    cache.current.clear()
    lastRefresh.current = ''
    setEntries([])
    setDirectories([''])
  }, [conversationId, projectRoot])

  useEffect(() => {
    if (!enabled || !conversationId) return
    let cancelled = false
    const refresh = lastRefresh.current !== refreshKey
    const paths = (JSON.parse(directoryKey) as string[]).filter(path => refresh || !cache.current.has(path))
    if (!paths.length) return
    setLoading(cache.current.size === 0)
    setError(null)
    void Promise.all(paths.map(async path => [path, await desktopClient.projectFilesChildren(conversationId, path)] as const))
      .then(rows => {
        if (cancelled) return
        if (refresh) cache.current.clear()
        rows.forEach(([path, response]) => cache.current.set(path, response.entries))
        lastRefresh.current = refreshKey
        setEntries([...cache.current.values()].flat())
      }).catch(cause => { if (!cancelled) setError(userFacingCause(cause)) })
      .finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [conversationId, projectRoot, enabled, directoryKey, refreshKey])

  useEffect(() => {
    if (!enabled || !conversationId || !query.trim()) { setResults([]); setTruncated(false); return }
    let cancelled = false
    setResults([])
    setSearchLoading(true)
    const timer = window.setTimeout(() => {
      setError(null)
      void desktopClient.projectFilesSearch(conversationId, query.trim(), 200).then(response => {
        if (!cancelled) { setResults(response.entries); setTruncated(response.truncated) }
      }).catch(cause => { if (!cancelled) setError(userFacingCause(cause)) })
        .finally(() => { if (!cancelled) setSearchLoading(false) })
    }, 200)
    return () => { cancelled = true; window.clearTimeout(timer) }
  }, [conversationId, projectRoot, enabled, query, refreshKey])

  return { entries, results, truncated, loading: loading || Boolean(query.trim() && searchLoading), error, setDirectories }
}
