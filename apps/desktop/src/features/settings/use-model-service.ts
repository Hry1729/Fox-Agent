import { useCallback, useEffect, useMemo, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { ModelConnectionTest, ModelServiceRecord } from '@/features/conversations/model/types'

export function useModelService() {
  const [service, setService] = useState<ModelServiceRecord | null>(null)
  const [loading, setLoading] = useState(desktopRuntimeAvailable)
  const [testing, setTesting] = useState(false)
  const [saving, setSaving] = useState(false)
  const [testResult, setTestResult] = useState<ModelConnectionTest | null>(null)
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    if (!desktopRuntimeAvailable) return
    setLoading(true)
    try {
      setService(await desktopClient.getModelService())
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    const changed = () => { void refresh() }
    changed()
    window.addEventListener('fox:model-service-changed', changed)
    return () => window.removeEventListener('fox:model-service-changed', changed)
  }, [refresh])

  const test = useCallback(async (baseUrl?: string, apiKey?: string, apiType?: 'openai-completions' | 'anthropic-messages', modelId?: string) => {
    setTesting(true)
    setError(null)
    try {
      const result = await desktopClient.testModelService(baseUrl, apiKey, apiType, modelId)
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

  const save = useCallback(async (request: { name: string; baseUrl: string; modelId: string; apiType: 'openai-completions' | 'anthropic-messages'; contextWindow: number; maxOutputTokens: number; supportsImageInput: boolean; apiKey?: string; clearApiKey?: boolean }) => {
    setSaving(true)
    setError(null)
    try {
      const saved = await desktopClient.saveModelService(request)
      setService(saved)
      window.dispatchEvent(new Event('fox:model-service-changed'))
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
