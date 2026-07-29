import { useCallback, useEffect, useMemo, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { YuxiModelRecord } from '@/features/conversations/model/types'

export function useYuxiModels(enabled = true) {
  const [models, setModels] = useState<YuxiModelRecord[]>([])
  const [loading, setLoading] = useState(false)

  const refresh = useCallback(async () => {
    if (!desktopRuntimeAvailable || !enabled) {
      setModels([])
      return []
    }
    setLoading(true)
    try {
      const next = await desktopClient.listYuxiModels()
      setModels(next)
      return next
    } catch {
      setModels([])
      return []
    } finally {
      setLoading(false)
    }
  }, [enabled])

  useEffect(() => { void refresh() }, [refresh])

  return useMemo(() => ({ models, loading, refresh }), [loading, models, refresh])
}
