import { useCallback, useEffect, useMemo, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import { normalizeAgentClassification, type ClassifiedAgentRecord } from './agent-classification'

export function useAgents() {
  const [agents, setAgents] = useState<ClassifiedAgentRecord[]>([])
  const [loading, setLoading] = useState(desktopRuntimeAvailable)
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(async (sync = true) => {
    if (!desktopRuntimeAvailable) return
    setLoading(true)
    try {
      if (sync) {
        try {
          await desktopClient.syncYuxiAgents()
        } catch {}
      }
      const records = await desktopClient.listAgents()
      setAgents(records.map(normalizeAgentClassification))
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { void refresh() }, [refresh])

  return useMemo(() => ({ agents, loading, error, refresh }), [agents, error, loading, refresh])
}
