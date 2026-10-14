import type { NotificationBridge } from './notification-center'
import { ToastStack, ToastStackProvider, useToastStack, type ToastType } from '@/components/arc/toast-stack/toast-stack'
import { AnimatedIconTrigger, Bell as AnimatedBell } from '@/components/fox-ui/animated-icons'
import { FoxEmptyState } from '@/components/fox-ui/empty-state'
import { SkeletonContent } from '@/components/ui/skeleton'
import { Progress } from '@/components/ui/progress'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { desktopClient, desktopErrorDetails, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import { type AppNotificationRecord, type NotificationPreferencesRecord } from '@/features/conversations/model/types'
import { FOX_IN_APP_NOTIFICATION_EVENT, FOX_NOTIFICATIONS_CHANGED_EVENT, FOX_NOTIFICATION_PREFERENCES_CHANGED_EVENT, type InAppNotificationEventDetail, normalizeNotificationSoundId, notificationIsVisibleInContext, playNotificationSound, notify as toast } from '@/features/notifications'
import { type NavigateWorkspace, type WorkspaceView } from '@/features/workspace/types'
import { AlertTriangle, Bell, Check, CircleDot, Eraser, ExternalLink, FolderOpen, MoreHorizontal, Pin, PinOff, RotateCcw } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

const NOTIFICATION_PIN_STORAGE_KEY = 'fox.notification.manually-pinned-ids'
type ToastNotification = Pick<AppNotificationRecord, 'id' | 'kind' | 'title' | 'body'> & Partial<AppNotificationRecord>
const toastType: Record<AppNotificationRecord['kind'], ToastType> = { completed: 'success', failed: 'error', progress: 'loading', approval: 'warning', question: 'info' }

function notificationFingerprint(item: AppNotificationRecord): string {
  return JSON.stringify([item.kind, item.severity, item.title, item.body, item.progress, item.status])
}

function notificationDay(item: AppNotificationRecord): { key: string; label: string } {
  const date = new Date(item.updatedAt)
  const today = new Date()
  const yesterday = new Date(today)
  yesterday.setDate(today.getDate() - 1)
  const key = `${date.getFullYear()}-${date.getMonth() + 1}-${date.getDate()}`
  const isSameDay = (value: Date) => date.getFullYear() === value.getFullYear()
    && date.getMonth() === value.getMonth()
    && date.getDate() === value.getDate()
  if (isSameDay(today)) return { key, label: '今天' }
  if (isSameDay(yesterday)) return { key, label: '昨天' }
  return { key, label: date.toLocaleDateString('zh-CN', { month: 'long', day: 'numeric', weekday: 'short' }) }
}

function loadManuallyPinnedNotificationIds(): Set<string> {
  if (typeof window === 'undefined') return new Set()
  try {
    const value = JSON.parse(window.localStorage.getItem(NOTIFICATION_PIN_STORAGE_KEY) ?? '[]')
    return new Set(Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [])
  } catch {
    return new Set()
  }
}

function isAutomaticPriorityNotification(item: AppNotificationRecord): boolean {
  return item.severity === 'high' || item.kind === 'approval' || item.kind === 'question' || item.kind === 'progress'
}

function NotificationCenterContent({
  onNavigate,
  open,
  onOpenChange,
  showTrigger,
  activeView,
  activeEntityId,
  activeConversationId,
  bridge,
}: {
  bridge: NotificationBridge
  onNavigate: NavigateWorkspace
  open: boolean
  onOpenChange: (open: boolean) => void
  showTrigger: boolean
  activeView: WorkspaceView
  activeEntityId?: string | null
  activeConversationId?: string | null
}) {
  const [unreadOnly, setUnreadOnly] = useState(false)
  const [items, setItems] = useState<AppNotificationRecord[]>([])
  const [preferences, setPreferences] = useState<NotificationPreferencesRecord | null>(null)
  const [loading, setLoading] = useState(desktopRuntimeAvailable)
  const [error, setError] = useState<string | null>(null)
  const { toast: showToast, dismiss: dismissToast } = useToastStack()
  const queuedFingerprints = useRef(new Map<string, string>())
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const [manuallyPinnedIds, setManuallyPinnedIds] = useState<Set<string>>(loadManuallyPinnedNotificationIds)
  const observedNotifications = useRef<Map<string, string> | null>(null)
  const loadGeneration = useRef(0)
  const openCenter = useRef(onOpenChange)
  openCenter.current = onOpenChange
  // Persistent record IDs retarget the same Arc card. Repeated refreshes must not restart its timer.
  const enqueueToast = useCallback((notification: ToastNotification) => {
    const fingerprint = JSON.stringify([notification.kind, notification.severity, notification.title, notification.body, notification.progress, notification.status])
    if (queuedFingerprints.current.get(notification.id) === fingerprint) return
    queuedFingerprints.current.set(notification.id, fingerprint)
    while (queuedFingerprints.current.size > 500) {
      const oldest = queuedFingerprints.current.keys().next().value
      if (oldest === undefined) break
      queuedFingerprints.current.delete(oldest)
    }
    showToast({
      id: notification.id,
      type: toastType[notification.kind],
      title: notification.title,
      description: notification.body,
      // Background progress should also leave the screen after a short wait; history stays in the center.
      duration: notification.kind === 'failed' || notification.kind === 'approval' ? 8000 : 5000,
      action: notification.sourceType ? { label: '查看', onClick: () => {
        openCenter.current(true)
        setUnreadOnly(false)
        setExpandedId(notification.id)
        if (notification.readAt == null) void desktopClient.setAppNotificationRead(notification.id, true)
          .then(() => window.dispatchEvent(new Event(FOX_NOTIFICATIONS_CHANGED_EVENT)))
          .catch(cause => toast.error('标记通知已读失败', { description: desktopErrorDetails(cause).message }))
      } } : undefined,
    })
  }, [showToast])

  const clearToastQueue = useCallback(() => dismissToast(), [dismissToast])

  const load = useCallback(async (quiet = false) => {
    const generation = ++loadGeneration.current
    if (!desktopRuntimeAvailable) {
      setItems([])
      setError(null)
      return
    }
    if (!quiet) setLoading(true)
    try {
      if (observedNotifications.current === null && bridge.initial) {
        const baseline = await bridge.initial
        if (generation !== loadGeneration.current) return
        if (baseline) {
          observedNotifications.current = new Map(baseline.map(item => [item.id, notificationFingerprint(item)]))
          setItems(baseline)
        }
      }
      const [nextItems, nextPreferences] = await Promise.all([
        desktopClient.listAppNotifications(unreadOnly, 100),
        desktopClient.getNotificationPreferences(),
      ])
      if (generation !== loadGeneration.current) return
      const visibleUnreadIds = new Set(nextItems
        .filter((item) => item.readAt == null && notificationIsVisibleInContext(item, {
          appVisible: document.visibilityState === 'visible',
          appFocused: document.hasFocus(),
          activeView,
          activeEntityId,
          activeConversationId,
        }))
        .map((item) => item.id))
      if (visibleUnreadIds.size) {
        const ids = [...visibleUnreadIds]
        const results = await Promise.allSettled(ids.map((id) => desktopClient.setAppNotificationRead(id, true)))
        results.forEach((result, index) => { if (result.status === 'rejected') visibleUnreadIds.delete(ids[index]) })
      }
      if (generation !== loadGeneration.current) return
      const seenAt = Date.now()
      const presentedItems = nextItems.map((item) => visibleUnreadIds.has(item.id) ? { ...item, readAt: seenAt } : item)
      const nextObserved = new Map(presentedItems.map((item) => [item.id, notificationFingerprint(item)]))
      if (observedNotifications.current) {
        const changed = presentedItems.filter((item) => item.readAt == null && observedNotifications.current?.get(item.id) !== notificationFingerprint(item))
        const shouldAlert = changed.filter((item) => !(nextPreferences.quietProgress && item.kind === 'progress'))
        shouldAlert.slice(0, 12).reverse().forEach((item) => enqueueToast(item))
        if (shouldAlert.length && nextPreferences.sound) playNotificationSound(normalizeNotificationSoundId(nextPreferences.soundId))
        if (shouldAlert.length && nextPreferences.systemPopup && typeof Notification !== 'undefined' && Notification.permission === 'granted') {
          const first = shouldAlert[0]
          new Notification(first.title, { body: shouldAlert.length > 1 ? `${first.body}（另有 ${shouldAlert.length - 1} 条更新）` : first.body, tag: first.mergeKey })
        }
        for (const [id, fingerprint] of nextObserved) observedNotifications.current.set(id, fingerprint)
        while (observedNotifications.current.size > 500) {
          const oldestId = observedNotifications.current.keys().next().value
          if (!oldestId) break
          observedNotifications.current.delete(oldestId)
        }
      } else {
        observedNotifications.current = nextObserved
      }
      setItems(presentedItems)
      setPreferences(nextPreferences)
      setError(null)
    } catch (cause) {
      if (generation === loadGeneration.current) setError(desktopErrorDetails(cause).message)
    } finally {
      if (generation === loadGeneration.current) setLoading(false)
    }
  }, [activeConversationId, activeEntityId, activeView, bridge, enqueueToast, unreadOnly])

  useEffect(() => () => { loadGeneration.current += 1 }, [load])

  useEffect(() => {
    if (!desktopRuntimeAvailable) return
    void load(observedNotifications.current !== null)
    const timer = window.setInterval(() => void load(true), open ? 5000 : 30000)
    return () => window.clearInterval(timer)
  }, [load, open])

  useEffect(() => {
    let refreshTimer: number | null = null
    const disconnect = bridge.connect({
      preview: detail => {
        enqueueToast(detail.notification)
        if (detail.forceSound) playNotificationSound(normalizeNotificationSoundId(detail.soundId))
      },
      preferences: setPreferences,
      refresh: () => {
        if (refreshTimer !== null) window.clearTimeout(refreshTimer)
        refreshTimer = window.setTimeout(() => { refreshTimer = null; void load(true) }, 80)
      },
      failure: () => setError('实时通知连接失败，将继续定时同步；也可以手动刷新通知。'),
    })
    return () => { disconnect(); if (refreshTimer !== null) window.clearTimeout(refreshTimer) }
  }, [bridge, enqueueToast, load])

  const unreadCount = items.filter((item) => item.readAt == null).length
  const hasReadNotifications = items.some((item) => item.readAt != null)
  const priorityItems = useMemo(() => items.filter((item) => manuallyPinnedIds.has(item.id) || isAutomaticPriorityNotification(item)), [items, manuallyPinnedIds])
  const datedItems = useMemo(() => items.filter((item) => !manuallyPinnedIds.has(item.id) && !isAutomaticPriorityNotification(item)), [items, manuallyPinnedIds])
  const notificationGroups = useMemo(() => datedItems.reduce<Array<{ key: string; label: string; items: AppNotificationRecord[] }>>((groups, item) => {
    const day = notificationDay(item)
    const current = groups.at(-1)
    if (current?.key === day.key) current.items.push(item)
    else groups.push({ ...day, items: [item] })
    return groups
  }, []), [datedItems])
  const toggleManualPin = useCallback((id: string) => {
    setManuallyPinnedIds((current) => {
      const next = new Set(current)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      window.localStorage.setItem(NOTIFICATION_PIN_STORAGE_KEY, JSON.stringify([...next]))
      return next
    })
  }, [])
  const inspectItem = async (item: AppNotificationRecord) => {
    setExpandedId((current) => current === item.id ? null : item.id)
    try {
      if (item.readAt == null) await desktopClient.setAppNotificationRead(item.id, true)
      dismissToast(item.id)
      await load(true)
    } catch (cause) {
      toast.error('标记通知已读失败', { description: desktopErrorDetails(cause).message })
    }
  }
  const openItem = async (item: AppNotificationRecord) => {
    try {
      if (item.readAt == null) await desktopClient.setAppNotificationRead(item.id, true)
      dismissToast(item.id)
      if (item.workspaceView) {
        onNavigate(item.workspaceView as WorkspaceView, item.entityId ?? undefined)
        onOpenChange(false)
      }
      await load(true)
    } catch (cause) {
      toast.error('打开通知失败', { description: desktopErrorDetails(cause).message })
    }
  }
  const openNotificationFile = async (item: AppNotificationRecord, reveal: boolean) => {
    const path = typeof item.action.path === 'string' ? item.action.path : null
    if (!path) return
    try {
      await desktopClient.openDownloadedFile(path, reveal)
    } catch (cause) {
      toast.error(reveal ? '无法打开所在文件夹' : '无法打开文件', { description: desktopErrorDetails(cause).message })
    }
  }
  const markAllRead = async () => {
    try {
      await desktopClient.markAllAppNotificationsRead()
      clearToastQueue()
      await load(true)
    } catch (cause) {
      toast.error('全部标记已读失败', { description: desktopErrorDetails(cause).message })
    }
  }
  const clearRead = async () => {
    try {
      await desktopClient.clearReadAppNotifications()
      clearToastQueue()
      setExpandedId(null)
      await load(true)
    } catch (cause) {
      toast.error('清除已读通知失败', { description: desktopErrorDetails(cause).message })
    }
  }
  const renderNotificationItem = (item: AppNotificationRecord) => {
    const expanded = expandedId === item.id
    const manuallyPinned = manuallyPinnedIds.has(item.id)
    const automaticPriority = isAutomaticPriorityNotification(item)
    const downloadedFile = item.action.type === 'downloaded_file' && typeof item.action.path === 'string'
    const canOpenDirectly = downloadedFile && item.action.canOpenDirectly === true
    return <article key={item.id} className={`fox-notification-entry ${expanded ? 'is-expanded' : ''}`}>
      <div className={`fox-notification-item fox-sidebar-tree-row is-${item.kind} ${item.readAt == null ? 'is-unread' : ''} ${expanded ? 'is-active' : ''} ${manuallyPinned ? 'is-manually-pinned' : ''}`}>
        <button type="button" className="fox-notification-item-main" onClick={() => void inspectItem(item)} aria-expanded={expanded}>
          <span className="fox-notification-dot" />
          <span className="fox-notification-copy"><span className="fox-notification-line"><strong>{item.title}</strong><time>{new Date(item.updatedAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}</time></span><small>{item.body}</small>{item.progress != null && item.kind === 'progress' && <Progress value={item.progress} aria-label={`${item.title}进度`} className="mt-1 h-1" />}</span>
        </button>
        {!automaticPriority && <button type="button" className="fox-notification-pin" aria-label={manuallyPinned ? `取消置顶 ${item.title}` : `置顶 ${item.title}`} aria-pressed={manuallyPinned} title={manuallyPinned ? '取消置顶' : '置顶'} onClick={() => toggleManualPin(item.id)}>{manuallyPinned ? <PinOff /> : <Pin />}</button>}
      </div>
      {expanded && <div className="fox-notification-details">
        <pre>{item.body}</pre>
        <footer><span>{item.sourceType} · {item.status}</span><div>{canOpenDirectly && <Button variant="outline" size="sm" onClick={() => void openNotificationFile(item, false)}><ExternalLink />打开文件</Button>}{downloadedFile && <Button variant="outline" size="sm" onClick={() => void openNotificationFile(item, true)}><FolderOpen />打开所在文件夹</Button>}{item.workspaceView && <Button variant="outline" size="sm" onClick={() => void openItem(item)}><ExternalLink />打开相关页面</Button>}</div></footer>
      </div>}
    </article>
  }
  return <>
    {showTrigger && <AnimatedIconTrigger><button type="button" className={`fox-notification-trigger ${open ? 'is-active' : ''}`} aria-label={unreadCount ? `${unreadCount} 条未读通知` : '通知'} aria-pressed={open} title="通知" onClick={() => onOpenChange(!open)}>
        <AnimatedBell />{preferences?.badge !== false && unreadCount > 0 && <span aria-hidden="true" />}
      </button></AnimatedIconTrigger>}
    {open && <section className="fox-notification-sidebar fox-sidebar-section" aria-label="通知">
      <header className="fox-section-head">
        <span>通知</span>
        <span>
          <Tooltip><TooltipTrigger asChild><Button variant="ghost" size="icon" className="fox-notification-clear-read" aria-label="清除已读通知" disabled={!hasReadNotifications} onClick={() => void clearRead()}><Eraser /></Button></TooltipTrigger><TooltipContent side="right"><p>清除已读</p></TooltipContent></Tooltip>
          <DropdownMenu>
          <DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-notification-more" aria-label="管理通知"><MoreHorizontal /></Button></DropdownMenuTrigger>
          <DropdownMenuContent align="end" side="right" sideOffset={6} className="fox-notification-menu">
            <DropdownMenuItem onSelect={() => setUnreadOnly((value) => !value)}>{unreadOnly ? <Bell /> : <CircleDot />}{unreadOnly ? '全部通知' : '只看未读'}</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => void load()}><RotateCcw />刷新通知</DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem disabled={!unreadCount} onSelect={() => void markAllRead()}><Check />全部已读</DropdownMenuItem>
          </DropdownMenuContent>
          </DropdownMenu>
        </span>
      </header>
      <div className="fox-notification-list" aria-live="polite">
        {error && items.length > 0 && <div className="fox-notification-error" role="alert"><AlertTriangle />{error}</div>}
        {loading && !items.length && <div className="p-3"><SkeletonContent label="正在同步任务状态" avatar lines={3} /></div>}
        {!loading && !items.length && !error && <FoxEmptyState title={unreadOnly ? '没有未读通知' : '暂无通知'} description={unreadOnly ? '已读记录仍保存在通知中心。' : '任务更新、审批和操作结果会显示在这里。'} action={unreadOnly ? <Button variant="outline" size="sm" onClick={() => setUnreadOnly(false)}>查看全部通知</Button> : undefined} />}
        {!loading && !items.length && error && <FoxEmptyState variant="error" title="通知同步失败" description={error} action={<Button variant="outline" size="sm" onClick={() => void load()}>重试</Button>} />}
        {items.length > 0 && <section className="fox-notification-day is-priority" aria-labelledby="notification-priority-title">
          <div className="fox-notification-day-title" id="notification-priority-title">置顶</div>
          <div className="fox-notification-day-items">
            {priorityItems.length ? priorityItems.map(renderNotificationItem) : <p className="fox-notification-priority-empty">暂无置顶通知</p>}
          </div>
        </section>}
        {notificationGroups.map((group) => <section className="fox-notification-day" key={group.key} aria-labelledby={`notification-day-${group.key}`}>
          <div className="fox-notification-day-title" id={`notification-day-${group.key}`}>{group.label}</div>
          <div className="fox-notification-day-items">
            {group.items.map(renderNotificationItem)}
          </div>
        </section>)}
      </div>
    </section>}
    <ToastStack label="通知更新" visibleToasts={3} />
  </>
}

export function NotificationCenter(props: Parameters<typeof NotificationCenterContent>[0]) {
  return <ToastStackProvider duration={5000} limit={12}><NotificationCenterContent {...props} /></ToastStackProvider>
}
