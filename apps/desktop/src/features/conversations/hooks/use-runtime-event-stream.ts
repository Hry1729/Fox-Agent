import { useEffect, type Dispatch, type MutableRefObject, type SetStateAction } from 'react'
import { flushSync } from 'react-dom'
import { desktopClient, desktopRuntimeAvailable } from '../api/desktop-client'
import { loadConversationWithRetry } from '../model/conversation-reload'
import { enqueueRuntimeEvent, takeRuntimeEventFrame } from '../model/runtime-event-queue'
import { mergeConversationDetail, reduceRuntimeNotifications } from '../model/runtime-event-reducer'
import type { ConversationDetail, DesktopErrorDetails, RuntimeEventNotification } from '../model/types'

interface RuntimeEventStreamOptions {
  authoritativeRunIdRef: MutableRefObject<string | null>
  activeConversationIdRef: MutableRefObject<string | null>
  runtimeListenerReadyRef: MutableRefObject<Promise<void> | null>
  runtimeEventQueueRef: MutableRefObject<RuntimeEventNotification[]>
  runtimeEventFrameRef: MutableRefObject<number | null>
  lastRuntimeEventAtRef: MutableRefObject<number>
  uiDispatchStartedRef: MutableRefObject<Map<string, number>>
  runtimeEventReceivedAtRef: MutableRefObject<Map<string, number>>
  setDetail: Dispatch<SetStateAction<ConversationDetail | null>>
  setError: Dispatch<SetStateAction<string | null>>
  setErrorDetails: Dispatch<SetStateAction<DesktopErrorDetails | null>>
  refreshList: () => Promise<unknown>
  /**
   * Every notification, including the active run's own events, so per-conversation
   * activity can be tracked without subscribing to the same Tauri event twice.
   */
  onRunActivity?: (notification: RuntimeEventNotification) => void
}

const terminalEventTypes = new Set([
  'run.completed',
  'run.cancelled',
  'run.failed',
  'run.interrupted',
])

export function useRuntimeEventStream(options: RuntimeEventStreamOptions) {
  const {
    authoritativeRunIdRef,
    activeConversationIdRef,
    runtimeListenerReadyRef,
    runtimeEventQueueRef,
    runtimeEventFrameRef,
    lastRuntimeEventAtRef,
    uiDispatchStartedRef,
    runtimeEventReceivedAtRef,
    setDetail,
    setError,
    setErrorDetails,
    refreshList,
    onRunActivity,
  } = options

  useEffect(() => {
    if (!desktopRuntimeAvailable) return
    let disposed = false
    let unlisten: (() => void) | undefined
    const flushRuntimeEvents = () => {
      runtimeEventFrameRef.current = null
      if (disposed || runtimeEventQueueRef.current.length === 0) return
      const notifications = takeRuntimeEventFrame(runtimeEventQueueRef.current)
        .filter((notification) => notification.runId !== authoritativeRunIdRef.current)
      flushSync(() => {
        setDetail((current) => current ? reduceRuntimeNotifications(current, notifications) : current)
      })
      const terminal = [...notifications].reverse().find(({ event }) => terminalEventTypes.has(event.type))
      if (runtimeEventQueueRef.current.length > 0) scheduleRuntimeFlush()
      if (!terminal) return

      const event = terminal.event
      const metricKey = `${terminal.runId}:${terminal.seq}`
      const terminalReceivedAt = runtimeEventReceivedAtRef.current.get(metricKey)
      runtimeEventReceivedAtRef.current.delete(metricKey)
      if (terminalReceivedAt !== undefined) {
        void desktopClient.recordUiMetric(
          terminal.runId,
          'ui.terminal_render',
          Math.max(0, Math.round(performance.now() - terminalReceivedAt)),
        ).catch(() => undefined)
      }

      const isCurrentConversation = activeConversationIdRef.current === terminal.conversationId
      if (isCurrentConversation
        && (event.type === 'run.failed' || event.type === 'run.interrupted')
        && typeof event.message === 'string') {
        setError(event.message)
        setErrorDetails({
          code: typeof event.code === 'string' ? event.code : 'runtime.failed',
          message: event.message,
          retryable: event.type === 'run.interrupted',
        })
      }
      if (!isCurrentConversation) {
        void refreshList()
        return
      }

      void loadConversationWithRetry(
        (conversationId) => desktopClient.loadConversation(conversationId),
        terminal.conversationId,
      ).then((persisted) => {
        if (disposed || activeConversationIdRef.current !== terminal.conversationId) return
        flushSync(() => setDetail((current) => mergeConversationDetail(persisted, current)))
      }).catch((cause) => {
        if (disposed || activeConversationIdRef.current !== terminal.conversationId) return
        const message = cause instanceof Error ? cause.message : String(cause)
        setError(`对话已结束，但重新加载持久化结果失败：${message}`)
        setErrorDetails({ code: 'conversation.reload_failed', message, retryable: true })
      })
      void refreshList()
    }
    const scheduleRuntimeFlush = () => {
      if (runtimeEventFrameRef.current !== null) return
      runtimeEventFrameRef.current = window.requestAnimationFrame(flushRuntimeEvents)
    }
    const subscription = desktopClient.listenRuntimeEvents((notification) => {
      if (disposed) return
      onRunActivity?.(notification)
      if (notification.runId === authoritativeRunIdRef.current) return
      const event = notification.event
      if (event.type === 'run.started') {
        setError(null)
        setErrorDetails(null)
      }
      lastRuntimeEventAtRef.current = Date.now()
      if (terminalEventTypes.has(event.type)) {
        runtimeEventReceivedAtRef.current.set(
          `${notification.runId}:${notification.seq}`,
          performance.now(),
        )
      }
      const dispatchedAt = uiDispatchStartedRef.current.get(notification.conversationId)
      if (dispatchedAt !== undefined) {
        uiDispatchStartedRef.current.delete(notification.conversationId)
        void desktopClient.recordUiMetric(
          notification.runId,
          'ui.first_event',
          Math.max(0, Math.round(performance.now() - dispatchedAt)),
        ).catch(() => undefined)
      }
      enqueueRuntimeEvent(runtimeEventQueueRef.current, notification)
      scheduleRuntimeFlush()
    })
    runtimeListenerReadyRef.current = subscription.then((stop) => {
      if (disposed) stop()
      else unlisten = stop
    }).catch((cause) => {
      if (!disposed) setError(cause instanceof Error ? cause.message : String(cause))
    })
    return () => {
      disposed = true
      runtimeEventQueueRef.current = []
      runtimeEventReceivedAtRef.current.clear()
      uiDispatchStartedRef.current.clear()
      lastRuntimeEventAtRef.current = 0
      if (runtimeEventFrameRef.current !== null) {
        window.cancelAnimationFrame(runtimeEventFrameRef.current)
        runtimeEventFrameRef.current = null
      }
      unlisten?.()
      runtimeListenerReadyRef.current = null
    }
  }, [
    authoritativeRunIdRef,
    activeConversationIdRef,
    lastRuntimeEventAtRef,
    refreshList,
    runtimeEventFrameRef,
    runtimeEventQueueRef,
    runtimeEventReceivedAtRef,
    runtimeListenerReadyRef,
    setDetail,
    setError,
    setErrorDetails,
    uiDispatchStartedRef,
  ])
}
