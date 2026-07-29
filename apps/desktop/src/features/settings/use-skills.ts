import { useCallback, useEffect, useMemo, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { SkillRecord } from '@/features/conversations/model/types'

export function useSkills(agentId = 'fox-general') {
  const [items, setItems] = useState<SkillRecord[]>([])
  const [loading, setLoading] = useState(desktopRuntimeAvailable)
  const [error, setError] = useState<string | null>(null)
  const refresh = useCallback(async () => {
    if (!desktopRuntimeAvailable) return
    setLoading(true)
    try { setItems(await desktopClient.listSkills(agentId)); setError(null) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLoading(false) }
  }, [agentId])
  useEffect(() => { void refresh() }, [refresh])
  const setEnabled = useCallback(async (skillId: string, enabled: boolean) => {
    try { setItems(await desktopClient.setSkillEnabled(agentId, skillId, enabled)); setError(null); return true }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); return false }
  }, [agentId])
  return useMemo(() => ({ items, loading, error, refresh, setEnabled }), [error, items, loading, refresh, setEnabled])
}
