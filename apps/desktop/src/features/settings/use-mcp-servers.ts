import { useCallback, useEffect, useMemo, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { McpConnectionTest, McpServerRecord } from '@/features/conversations/model/types'

export interface SaveMcpServerInput {
  id?: string
  name: string
  command: string
  args: string[]
  environment?: Record<string, string>
  clearEnvironment?: boolean
}

export function useMcpServers() {
  const [items, setItems] = useState<McpServerRecord[]>([])
  const [tests, setTests] = useState<Record<string, McpConnectionTest>>({})
  const [loading, setLoading] = useState(desktopRuntimeAvailable)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const refresh = useCallback(async () => {
    if (!desktopRuntimeAvailable) return
    setLoading(true)
    try { setItems(await desktopClient.listMcpServers()); setError(null) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLoading(false) }
  }, [])
  useEffect(() => { void refresh() }, [refresh])
  const save = useCallback(async (input: SaveMcpServerInput) => {
    setBusyId(input.id ?? 'new')
    try { const saved = await desktopClient.saveMcpServer(input); await refresh(); setError(null); return saved }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); return null }
    finally { setBusyId(null) }
  }, [refresh])
  const test = useCallback(async (serverId: string) => {
    setBusyId(serverId)
    try { const result = await desktopClient.testMcpServer(serverId); setTests((value) => ({ ...value, [serverId]: result })); await refresh(); setError(null); return result }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); await refresh(); return null }
    finally { setBusyId(null) }
  }, [refresh])
  const setEnabled = useCallback(async (serverId: string, enabled: boolean) => {
    setBusyId(serverId)
    try { const updated = await desktopClient.setMcpServerEnabled(serverId, enabled); setItems((value) => value.map((item) => item.id === serverId ? updated : item)); setError(null); return true }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); return false }
    finally { setBusyId(null) }
  }, [])
  const remove = useCallback(async (serverId: string) => {
    setBusyId(serverId)
    try { await desktopClient.deleteMcpServer(serverId); setItems((value) => value.filter((item) => item.id !== serverId)); setTests((value) => { const next = { ...value }; delete next[serverId]; return next }); setError(null); return true }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); return false }
    finally { setBusyId(null) }
  }, [])
  return useMemo(() => ({ items, tests, loading, busyId, error, refresh, save, test, setEnabled, remove }), [busyId, error, items, loading, refresh, remove, save, setEnabled, test, tests])
}
