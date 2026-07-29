import { useCallback, useEffect, useMemo, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { YuxiUserRecord } from '@/features/conversations/model/types'

export function useYuxiUser(enabled = true) {
  const [user, setUser] = useState<YuxiUserRecord | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    if (!desktopRuntimeAvailable || !enabled) {
      setUser(null)
      setError(null)
      return null
    }
    setLoading(true)
    try {
      const next = await desktopClient.getYuxiUser()
      setUser(next)
      setError(null)
      return next
    } catch (cause) {
      setUser(null)
      setError(cause instanceof Error ? cause.message : String(cause))
      return null
    } finally {
      setLoading(false)
    }
  }, [enabled])

  useEffect(() => {
    void refresh()
    const onChanged = () => void refresh()
    window.addEventListener('fox:yuxi-user-changed', onChanged)
    return () => window.removeEventListener('fox:yuxi-user-changed', onChanged)
  }, [refresh])

  return useMemo(() => ({ user, loading, error, refresh }), [error, loading, refresh, user])
}
