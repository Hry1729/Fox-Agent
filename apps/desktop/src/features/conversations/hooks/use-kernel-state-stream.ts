import { useEffect, type Dispatch, type SetStateAction } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '../api/desktop-client'
import { createCoalescedRefresh } from '../model/coalesced-refresh'
import { mergeConversationDetail } from '../model/runtime-event-reducer'
import type { ConversationDetail, DesktopErrorDetails } from '../model/types'

export function useKernelStateStream({ conversationId, setDetail, setError, setErrorDetails }: {
  conversationId: string | null
  setDetail: Dispatch<SetStateAction<ConversationDetail | null>>
  setError: Dispatch<SetStateAction<string | null>>
  setErrorDetails: Dispatch<SetStateAction<DesktopErrorDetails | null>>
}) {
  useEffect(() => {
    if (!desktopRuntimeAvailable || !conversationId) return
    let disposed = false
    let unlisten: (() => void) | undefined
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
    void desktopClient.listenKernelStateInvalidations((notice) => {
      if (notice?.schemaVersion === 1) refresh.invalidate()
    }).then((stop) => {
      if (disposed) { stop(); return }
      unlisten = stop
      // Always read AFTER registration, including terminal runs. Closes the
      // window between opening a conversation and subscribing to new commits.
      refresh.invalidate()
    }).catch(report)
    return () => { disposed = true; refresh.dispose(); unlisten?.() }
  }, [conversationId, setDetail, setError, setErrorDetails])
}
