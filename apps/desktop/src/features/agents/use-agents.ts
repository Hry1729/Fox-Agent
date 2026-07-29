import { useCallback, useEffect, useMemo, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { AgentRecord } from '@/features/conversations/model/types'

export function useAgents() {
  const [agents, setAgents] = useState<AgentRecord[]>([])
  const [loading, setLoading] = useState(desktopRuntimeAvailable)
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(async (sync = true) => {
    if (!desktopRuntimeAvailable) return
    setLoading(true)
    try {
      let knowledgeServiceOnline = true
      if (sync) {
        try {
          await desktopClient.syncYuxiAgents()
        } catch {
          knowledgeServiceOnline = false
        }
      }
      const records = await desktopClient.listAgents()
      setAgents(knowledgeServiceOnline ? records : records.filter((agent) => agent.runtimeType !== 'yuxi'))
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
