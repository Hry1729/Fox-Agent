import { useEffect, type Dispatch, type SetStateAction } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '../api/desktop-client'
import { createCoalescedRefresh } from '../model/coalesced-refresh'
import { mergeConversationDetail } from '../model/runtime-event-reducer'
import { applyKernelModelPreview } from '../model/kernel-model-preview'
import type { ConversationDetail, DesktopErrorDetails } from '../model/types'

export function useKernelStateStream({ conversationId, active = false, setDetail, setError, setErrorDetails }: {
  conversationId: string | null
  active?: boolean
  setDetail: Dispatch<SetStateAction<ConversationDetail | null>>
  setError: Dispatch<SetStateAction<string | null>>
  setErrorDetails: Dispatch<SetStateAction<DesktopErrorDetails | null>>
}) {
  useEffect(() => {
    if (!desktopRuntimeAvailable || !conversationId) return
    let disposed = false
    let stopPreviews: (() => void) | undefined
    let ownError: string | null = null
    const report = (cause: unknown) => {
      if (disposed) return
      const reason = cause instanceof Error ? cause.message : String(cause)
      ownError = `运行状态同步失败，请重新打开对话：${reason}`
      setError(ownError)
      setErrorDetails({ code: 'kernel.state_refresh_failed', message: ownError, retryable: true })
    }
    const refresh = createCoalescedRefresh({
      load: () => desktopClient.loadConversation(conversationId),
      apply: (persisted) => {
        setDetail((current) => current?.conversation.id === conversationId
          ? mergeConversationDetail(persisted, current) : current)
        if (ownError) {
          const previousError = ownError
          setError((current) => current === previousError ? null : current)
          setErrorDetails((current) => current?.code === 'kernel.state_refresh_failed' ? null : current)
          ownError = null
        }
      },
      fail: report,
    })
    // Read the persisted state once on open, including terminal runs; while
    // the run is active the interval below keeps it current.
    refresh.invalidate()
    void desktopClient.listenKernelModelPreviews((notice) => {
      if (!disposed && notice?.conversationId === conversationId) {
        setDetail(current => applyKernelModelPreview(current, notice))
        refresh.invalidate()
      }
    }).then(stop => { if (disposed) stop(); else stopPreviews = stop }).catch(() => {
      // Transient display is optional; durable state still refreshes normally.
    })
    // Waiting telemetry is durable presentation data. Read it only while this
    // selected run is active, using the same serialized merge path.
    const timer = active ? window.setInterval(() => refresh.invalidate(), 2_500) : null
    return () => { disposed = true; if (timer !== null) window.clearInterval(timer); refresh.dispose(); stopPreviews?.() }
  }, [conversationId, active, setDetail, setError, setErrorDetails])
}
