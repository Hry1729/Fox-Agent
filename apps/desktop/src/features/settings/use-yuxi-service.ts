import { useCallback, useEffect, useMemo, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { YuxiConnectionTest, YuxiServiceRecord } from '@/features/conversations/model/types'

export function useYuxiService() {
  const [service, setService] = useState<YuxiServiceRecord | null>(null)
  const [loading, setLoading] = useState(desktopRuntimeAvailable)
  const [testing, setTesting] = useState(false)
  const [saving, setSaving] = useState(false)
  const [testResult, setTestResult] = useState<YuxiConnectionTest | null>(null)
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    if (!desktopRuntimeAvailable) return
    setLoading(true)
    try {
      setService(await desktopClient.getYuxiService())
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void refresh()
    const onUserChanged = () => void refresh()
    window.addEventListener('fox:yuxi-user-changed', onUserChanged)
    return () => window.removeEventListener('fox:yuxi-user-changed', onUserChanged)
  }, [refresh])

  const test = useCallback(async (baseUrl?: string, accessToken?: string) => {
    setTesting(true)
    setError(null)
    try {
      const result = await desktopClient.testYuxiService(baseUrl, accessToken)
      setTestResult(result)
      if (!baseUrl || baseUrl === service?.baseUrl) await refresh()
      return result
    } catch (cause) {
      setTestResult(null)
      setError(cause instanceof Error ? cause.message : String(cause))
      return null
    } finally {
      setTesting(false)
    }
  }, [refresh, service?.baseUrl])

  const save = useCallback(async (request: { name: string; baseUrl: string; accessToken?: string; clearAccessToken?: boolean }) => {
    setSaving(true)
    setError(null)
    try {
      const saved = await desktopClient.saveYuxiService(request)
      setService(saved)
      return saved
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      return null
    } finally {
      setSaving(false)
    }
  }, [])

  return useMemo(() => ({ enabled: desktopRuntimeAvailable, service, loading, testing, saving, testResult, error, refresh, test, save }), [error, loading, refresh, save, saving, service, test, testResult, testing])
}
