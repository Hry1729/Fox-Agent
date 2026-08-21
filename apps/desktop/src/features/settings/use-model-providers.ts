import { useCallback, useEffect, useMemo, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { ModelProviderRecord } from '@/features/conversations/model/types'

export type SaveModelProviderInput = {
  id?: string
  name: string
  icon?: string | null
  baseUrl: string
  apiType: 'openai-completions' | 'anthropic-messages'
  isDefault: boolean
  models: Array<{ id?: string; modelId: string; displayName: string; contextWindow: number; maxOutputTokens: number; supportsImageInput: boolean; isDefault: boolean }>
  apiKey?: string
  clearApiKey?: boolean
}

export function useModelProviders() {
  const [items, setItems] = useState<ModelProviderRecord[]>([])
  const [loading, setLoading] = useState(desktopRuntimeAvailable)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    if (!desktopRuntimeAvailable) return
    setLoading(true)
    try { setItems(await desktopClient.listModelProviders()); setError(null) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLoading(false) }
  }, [])

  useEffect(() => { void refresh() }, [refresh])

  const save = useCallback(async (request: SaveModelProviderInput) => {
    setBusyId(request.id ?? 'new'); setError(null)
    try { const saved = await desktopClient.saveModelProvider(request); await refresh(); return saved }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); return null }
    finally { setBusyId(null) }
  }, [refresh])

  const remove = useCallback(async (providerId: string) => {
    setBusyId(providerId); setError(null)
    try { const deleted = await desktopClient.deleteModelProvider(providerId); await refresh(); return deleted }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); return false }
    finally { setBusyId(null) }
  }, [refresh])

  return useMemo(() => ({ items, loading, busyId, error, refresh, save, remove }), [busyId, error, items, loading, refresh, remove, save])
}
