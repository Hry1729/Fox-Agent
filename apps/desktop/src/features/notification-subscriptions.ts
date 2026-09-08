import type { desktopClient } from './conversations/api/desktop-client'
import { runtimeEventRefreshesNotifications } from './notifications'

type NotificationClient = Pick<typeof desktopClient,
  'listenRuntimeEvents' | 'listenWorkEvents' | 'listenApprovalRequests' | 'listenApprovalResolved' | 'listenKernelStateInvalidations'>

/** Independent registrations: one failed channel must not leak the others. */
export function subscribeNotificationRefresh(client: NotificationClient, refresh: () => void, fail: (cause: unknown) => void) {
  let disposed = false
  const stops: Array<() => void> = []
  const register = (subscribe: () => Promise<() => void>) => {
    void Promise.resolve().then(subscribe).then((stop) => {
      if (disposed) stop()
      else { stops.push(stop); refresh() }
    }).catch((cause) => { if (!disposed) fail(cause) })
  }
  const invalidate = () => { if (!disposed) refresh() }
  register(() => client.listenRuntimeEvents((notice) => {
    if (runtimeEventRefreshesNotifications(notice.event.type)) invalidate()
  }))
  register(() => client.listenWorkEvents(invalidate))
  register(() => client.listenApprovalRequests(invalidate))
  register(() => client.listenApprovalResolved(invalidate))
  register(() => client.listenKernelStateInvalidations((notice) => {
    if (notice?.schemaVersion === 1) invalidate()
  }))
  return () => { disposed = true; stops.splice(0).forEach((stop) => stop()) }
}
