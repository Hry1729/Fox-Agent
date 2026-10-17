import '@/components/fox-ui/sidebar-list-pane.css'
import { useLayoutEffect, useRef } from 'react'
import { Bell } from 'lucide-react'
import { deferredModule } from '@/components/fox-ui/use-deferred-module'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { AppNotificationRecord, NotificationPreferencesRecord } from '@/features/conversations/model/types'
import type { NavigateWorkspace, WorkspaceView } from '@/features/workspace/types'
import { subscribeNotificationRefresh } from '@/features/notification-subscriptions'
import { FOX_IN_APP_NOTIFICATION_EVENT, FOX_NOTIFICATION_PREFERENCES_CHANGED_EVENT, FOX_NOTIFICATIONS_CHANGED_EVENT, type InAppNotificationEventDetail } from '@/features/notifications'

export interface NotificationCenterProps {
  onNavigate: NavigateWorkspace
  open: boolean
  onOpenChange: (open: boolean) => void
  showTrigger: boolean
  activeView: WorkspaceView
  activeEntityId?: string | null
  activeConversationId?: string | null
}
type Handlers = { preview: (detail: InAppNotificationEventDetail) => void; preferences: (value: NotificationPreferencesRecord) => void; refresh: () => void; failure: () => void }
export interface NotificationBridge {
  initial: Promise<AppNotificationRecord[] | null> | null
  connect: (handlers: Handlers) => () => void
}
const implementation = deferredModule(() => import('./notification-center-implementation'))
/** Listeners live at the compatibility entry, including while the Arc chunk is in flight. */
export function NotificationCenter(props: NotificationCenterProps) {
  const loaded = implementation.useModule()
  const state = useRef<{ handlers?: Handlers; previews: InAppNotificationEventDetail[]; preferences?: NotificationPreferencesRecord; refresh: boolean; failed: boolean }>({ previews: [], refresh: false, failed: false })
  const bridge = useRef<NotificationBridge>({ initial: null, connect: handlers => {
    const current = state.current
    current.handlers = handlers
    current.previews.splice(0).forEach(handlers.preview)
    if (current.preferences) { handlers.preferences(current.preferences); current.preferences = undefined }
    if (current.refresh) { current.refresh = false; handlers.refresh() }
    if (current.failed) { current.failed = false; handlers.failure() }
    return () => { if (current.handlers === handlers) current.handlers = undefined }
  } })
  useLayoutEffect(() => {
    const current = state.current
    const preview = (event: Event) => {
      const detail = (event as CustomEvent<InAppNotificationEventDetail>).detail
      if (!detail?.notification) return
      if (current.handlers) current.handlers.preview(detail)
      else current.previews.push(detail)
      implementation.preload()
    }
    const preferences = (event: Event) => {
      const detail = (event as CustomEvent<NotificationPreferencesRecord>).detail
      if (current.handlers) current.handlers.preferences(detail)
      else current.preferences = detail
    }
    const refresh = () => { if (current.handlers) current.handlers.refresh(); else current.refresh = true }
    const failure = () => { if (current.handlers) current.handlers.failure(); else current.failed = true }
    const visible = () => { if (document.visibilityState === 'visible') refresh() }
    window.addEventListener(FOX_IN_APP_NOTIFICATION_EVENT, preview)
    window.addEventListener(FOX_NOTIFICATION_PREFERENCES_CHANGED_EVENT, preferences)
    window.addEventListener(FOX_NOTIFICATIONS_CHANGED_EVENT, refresh)
    window.addEventListener('focus', refresh)
    document.addEventListener('visibilitychange', visible)
    // Capture the baseline now, so updates during chunk loading are not mistaken for initial history.
    if (desktopRuntimeAvailable) bridge.current.initial = desktopClient.listAppNotifications(false, 100).catch(() => null)
    const stop = desktopRuntimeAvailable ? subscribeNotificationRefresh(desktopClient, refresh, failure) : undefined
    return () => {
      window.removeEventListener(FOX_IN_APP_NOTIFICATION_EVENT, preview)
      window.removeEventListener(FOX_NOTIFICATION_PREFERENCES_CHANGED_EVENT, preferences)
      window.removeEventListener(FOX_NOTIFICATIONS_CHANGED_EVENT, refresh)
      window.removeEventListener('focus', refresh)
      document.removeEventListener('visibilitychange', visible)
      stop?.()
    }
  }, [])
  if (loaded) return <loaded.NotificationCenter {...props} bridge={bridge.current} />
  return <>
    {props.showTrigger && <button type="button" className={`fox-notification-trigger ${props.open ? 'is-active' : ''}`} aria-label="通知" aria-pressed={props.open} title="通知" onPointerEnter={implementation.preload} onFocus={implementation.preload} onClick={() => { implementation.preload(); props.onOpenChange(!props.open) }}><Bell /></button>}
    {props.open && <section className="fox-notification-sidebar fox-sidebar-list-pane" aria-label="通知"><header className="fox-section-head"><span>通知</span></header><div className="fox-notification-list" role="status" aria-label="正在同步任务状态" /></section>}
  </>
}
