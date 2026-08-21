import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import { normalizeAgentClassification, type ClassifiedAgentRecord } from './agent-classification'

export function useAgents() {
  const [agents, setAgents] = useState<ClassifiedAgentRecord[]>([])
  const [loading, setLoading] = useState(desktopRuntimeAvailable)
  const [syncing, setSyncing] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [syncError, setSyncError] = useState<string | null>(null)
  const mountedRef = useRef(false)
  const loadedRef = useRef(false)
  const syncTaskRef = useRef<Promise<void> | null>(null)

  const syncInBackground = useCallback(() => {
    if (!desktopRuntimeAvailable || syncTaskRef.current) return
    if (mountedRef.current) {
      setSyncing(true)
      setSyncError(null)
    }

    const syncTask = (async () => {
      try {
        await desktopClient.syncYuxiAgents()
      } catch (cause) {
        console.warn('Yuxi agent sync failed; using cached agents.', cause)
        if (mountedRef.current) {
          setSyncError(cause instanceof Error ? cause.message : String(cause))
        }
      }

      try {
        const records = await desktopClient.listAgents()
        if (mountedRef.current) {
          setAgents(records.map(normalizeAgentClassification))
          setError(null)
        }
      } catch (cause) {
        if (mountedRef.current) {
          setError(cause instanceof Error ? cause.message : String(cause))
        }
      } finally {
        if (mountedRef.current) setSyncing(false)
      }
    })()

    syncTaskRef.current = syncTask
    void syncTask.finally(() => {
      if (syncTaskRef.current === syncTask) syncTaskRef.current = null
    })
  }, [])

  const refresh = useCallback(async (sync = true) => {
    if (!desktopRuntimeAvailable) return
    if (!loadedRef.current && mountedRef.current) setLoading(true)
    try {
      const records = await desktopClient.listAgents()
      if (mountedRef.current) {
        setAgents(records.map(normalizeAgentClassification))
        setError(null)
        loadedRef.current = true
      }
    } catch (cause) {
      if (mountedRef.current) {
        setError(cause instanceof Error ? cause.message : String(cause))
      }
    } finally {
      if (mountedRef.current) setLoading(false)
    }

    if (sync && mountedRef.current) syncInBackground()
  }, [syncInBackground])

  useEffect(() => {
    mountedRef.current = true
    void refresh()
    return () => { mountedRef.current = false }
  }, [refresh])

  return useMemo(
    () => ({ agents, loading, syncing, error, syncError, refresh }),
    [agents, error, loading, refresh, syncError, syncing],
  )
}
