import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { ApiResponse } from '@/features/conversations/model/types'
import type { JobSnapshot } from '@/features/jobs/job-presentation'
import type { ContextBudgetDto } from '../components/context-budget-presentation'

export interface RuntimeContextWindow {
  usedTokens: number
  limitTokens: number
  source: 'session' | 'last_request'
  systemTokens?: number
  toolsTokens?: number
}

interface ReliabilitySnapshot { runId: string | null; contextBudget: ContextBudgetDto | null; contextWindow?: RuntimeContextWindow | null; jobs: JobSnapshot[] }

export function useContextBudget(conversationId: string | undefined, running: boolean, enabled = true) {
  const [loaded, setLoaded] = useState<{ id: string; snapshot: ReliabilitySnapshot } | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    setError(null)
    if (!conversationId || !desktopRuntimeAvailable || !enabled) return
    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | undefined
    let loading = false
    const visible = () => document.visibilityState !== 'hidden'
    const load = async () => {
      if (cancelled || !visible() || loading) return
      loading = true
      try {
        const response = await invoke<ApiResponse<ReliabilitySnapshot>>('kernel_reliability_snapshot', { request: { conversationId } })
        if (!response.ok) throw new Error(response.error.message)
        if (!cancelled) { setLoaded({ id: conversationId, snapshot: response.data }); setError(null) }
      } catch (cause) { if (!cancelled) setError(String(cause)) }
      finally {
        loading = false
        if (!cancelled && visible()) timer = setTimeout(load, running ? 1500 : 5000)
      }
    }
    const visibility = () => { clearTimeout(timer); if (document.visibilityState !== 'hidden') void load() }
    document.addEventListener('visibilitychange', visibility)
    void load()
    return () => { cancelled = true; clearTimeout(timer); document.removeEventListener('visibilitychange', visibility) }
  }, [conversationId, running, enabled])
  const snapshot = loaded && loaded.id === conversationId ? loaded.snapshot : null
  return { snapshot, budget: snapshot?.contextBudget ?? null, error }
}
