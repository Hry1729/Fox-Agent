import { useCallback, useEffect, useMemo, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { LifecycleHookRecord, SaveLifecycleHookInput } from '@/features/conversations/model/types'

export function useLifecycleHooks() {
  const [items, setItems] = useState<LifecycleHookRecord[]>([])
  const [loading, setLoading] = useState(desktopRuntimeAvailable)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const refresh = useCallback(async () => {
    if (!desktopRuntimeAvailable) return
    setLoading(true)
    try { setItems(await desktopClient.listLifecycleHooks()); setError(null) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLoading(false) }
  }, [])
  useEffect(() => { void refresh() }, [refresh])
  const save = useCallback(async (input: SaveLifecycleHookInput) => {
    setBusyId(input.id ?? 'new-hook')
    try { const saved = await desktopClient.saveLifecycleHook(input); await refresh(); setError(null); return saved }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); return null }
    finally { setBusyId(null) }
  }, [refresh])
  const setEnabled = useCallback(async (hookId: string, enabled: boolean) => {
    setBusyId(hookId)
    try { const updated = await desktopClient.setLifecycleHookEnabled(hookId, enabled); setItems((value) => value.map((item) => item.id === hookId ? updated : item)); setError(null); return true }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); return false }
    finally { setBusyId(null) }
  }, [])
  const remove = useCallback(async (hookId: string) => {
    setBusyId(hookId)
    try { await desktopClient.deleteLifecycleHook(hookId); setItems((value) => value.filter((item) => item.id !== hookId)); setError(null); return true }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); return false }
    finally { setBusyId(null) }
  }, [])
  return useMemo(() => ({ items, loading, busyId, error, refresh, save, setEnabled, remove }), [busyId, error, items, loading, refresh, remove, save, setEnabled])
}
