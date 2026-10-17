import '@/components/fox-ui/sidebar-list-pane.css'
import type { NotificationBridge } from './notification-center'
import { createPortal } from 'react-dom'
import './notification-center.css'
import { ToastStack, ToastStackProvider, useToastStack, type ToastType } from '@/components/arc/toast-stack/toast-stack'
import { AnimatedIconTrigger, Bell as AnimatedBell } from '@/components/fox-ui/animated-icons'
import { FoxEmptyState } from '@/components/fox-ui/empty-state'
import { SkeletonContent } from '@/components/ui/skeleton'
import { Progress } from '@/components/ui/progress'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { desktopClient, desktopErrorDetails, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import { type AppNotificationRecord, type NotificationPreferencesRecord } from '@/features/conversations/model/types'
import { FOX_IN_APP_NOTIFICATION_EVENT, FOX_NOTIFICATIONS_CHANGED_EVENT, FOX_NOTIFICATION_PREFERENCES_CHANGED_EVENT, type InAppNotificationEventDetail, normalizeNotificationSoundId, notificationIsVisibleInContext, playNotificationSound, notify as toast } from '@/features/notifications'
import { type NavigateWorkspace, type WorkspaceView } from '@/features/workspace/types'
import { AlertTriangle, Bell, CheckCheck, CircleDot, Eraser, ExternalLink, FolderOpen, MoreHorizontal, RotateCcw, Bot, Database, ListTodo, MessageSquare, Settings } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

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

function isAutomaticPriorityNotification(item: AppNotificationRecord): boolean {
  return item.kind === 'failed' || item.kind === 'approval' || item.kind === 'question'
}

function notificationDestination(item: AppNotificationRecord) {
  const view = item.workspaceView ?? ''
  if (view.includes('knowledge') || view === 'local-files' || item.sourceType === 'knowledge') return { Icon: Database, label: '资料库', action: '打开资料库' }
  if (view === 'agents' || view === 'agent-detail') return { Icon: Bot, label: '专家', action: '查看专家' }
  if (view === 'chat' || item.sourceType === 'run') return { Icon: MessageSquare, label: '对话', action: '打开对话' }
  if (view.startsWith('settings') || view === 'model-service') return { Icon: Settings, label: '设置', action: '去设置' }
  return { Icon: ListTodo, label: '任务', action: '查看任务' }
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
  const priorityItems = useMemo(() => items.filter(isAutomaticPriorityNotification), [items])
  const datedItems = useMemo(() => items.filter((item) => !isAutomaticPriorityNotification(item)), [items])
  const notificationGroups = useMemo(() => datedItems.reduce<Array<{ key: string; label: string; items: AppNotificationRecord[] }>>((groups, item) => {
    const day = notificationDay(item)
    const current = groups.at(-1)
    if (current?.key === day.key) current.items.push(item)
    else groups.push({ ...day, items: [item] })
    return groups
  }, []), [datedItems])
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
    const { Icon, label, action } = notificationDestination(item)
    // Run records store the conversation title, rather than an event title.
    const title = item.sourceType === 'run'
      ? item.kind === 'failed' ? '任务运行失败'
        : item.status === 'cancelled' ? '任务已取消'
          : item.kind === 'completed' ? '任务已完成'
            : item.kind === 'approval' ? '等待确认'
              : item.kind === 'question' ? '等待回答'
                : item.status === 'queued' ? '任务正在排队' : '任务正在执行'
      : item.title
    const source = item.sourceType === 'run' ? item.title : label
    const status = item.kind === 'failed' ? '失败' : item.readAt == null ? '未读' : '已读'
    const downloadedFile = item.action.type === 'downloaded_file' && typeof item.action.path === 'string'
    const canOpenDirectly = downloadedFile && item.action.canOpenDirectly === true
    return <article key={item.id} className={`fox-notification-entry ${expanded ? 'is-expanded' : ''}`}>
      <div className={`fox-notification-item fox-sidebar-tree-row is-${item.kind} ${item.readAt == null ? 'is-unread' : ''} ${expanded ? 'is-active' : ''}`}>
        <button type="button" className="fox-notification-item-main" onClick={() => void inspectItem(item)} aria-expanded={expanded}>
          <span className="fox-notification-source-icon" role="img" aria-label={`${label}通知，${status}`}><Icon aria-hidden="true" />{item.kind === 'failed' ? <i className="fox-notification-badge is-failed" /> : item.readAt == null ? <i className="fox-notification-badge is-unread" /> : null}</span>
          <span className="fox-notification-copy"><span className="fox-notification-line"><strong>{title}</strong><time>{new Date(item.updatedAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}</time></span><small>{source}</small>{item.progress != null && item.kind === 'progress' && <Progress value={item.progress} aria-label={`${item.title}进度`} className="mt-1 h-1" />}</span>
        </button>

      </div>
      {expanded && <div className="fox-notification-details">
        <p>{item.body}</p>
        <footer><div>{canOpenDirectly && <Button variant="outline" size="sm" onClick={() => void openNotificationFile(item, false)}><ExternalLink />打开文件</Button>}{downloadedFile && <Button variant="outline" size="sm" onClick={() => void openNotificationFile(item, true)}><FolderOpen />打开所在文件夹</Button>}{item.workspaceView && <Button variant="outline" size="sm" onClick={() => void openItem(item)}><ExternalLink />{action}</Button>}</div></footer>
      </div>}
    </article>
  }
  return <>
    {showTrigger && <AnimatedIconTrigger><button type="button" className={`fox-notification-trigger ${open ? 'is-active' : ''}`} aria-label={unreadCount ? `${unreadCount} 条未读通知` : '通知'} aria-pressed={open} title="通知" onClick={() => onOpenChange(!open)}>
        <AnimatedBell />{preferences?.badge !== false && unreadCount > 0 && <span aria-hidden="true" />}
      </button></AnimatedIconTrigger>}
    {open && <section className="fox-notification-sidebar fox-sidebar-list-pane" aria-label="通知">
      <header className="fox-section-head">
        <span>通知</span>
        <span>
          <Button variant="ghost" size="icon" className="fox-icon-button fox-notification-mark-read" aria-label="全部已读" title="全部已读" disabled={!unreadCount} onClick={() => void markAllRead()}><CheckCheck size={14} /></Button>
          <DropdownMenu>
          <DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button fox-notification-more" aria-label="管理通知"><MoreHorizontal /></Button></DropdownMenuTrigger>
          <DropdownMenuContent align="end" side="right" sideOffset={6} className="fox-notification-menu">
            <DropdownMenuItem onSelect={() => setUnreadOnly((value) => !value)}>{unreadOnly ? <Bell /> : <CircleDot />}{unreadOnly ? '全部通知' : '只看未读'}</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => void load()}><RotateCcw />刷新通知</DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => { onNavigate('settings'); onOpenChange(false) }}><Settings />通知设置</DropdownMenuItem>
            <DropdownMenuItem disabled={!hasReadNotifications} onSelect={() => void clearRead()}><Eraser />清除已读</DropdownMenuItem>
          </DropdownMenuContent>
          </DropdownMenu>
        </span>
      </header>
      <ScrollArea className="fox-sidebar-scroll"><div className="fox-notification-list" aria-live="polite">
        {error && items.length > 0 && <div className="fox-notification-error" role="alert"><AlertTriangle />{error}</div>}
        {loading && !items.length && <div className="p-3"><SkeletonContent label="正在同步任务状态" avatar lines={3} /></div>}
        {!loading && !items.length && !error && <FoxEmptyState title={unreadOnly ? '没有未读通知' : '暂无通知'} description={unreadOnly ? '已读记录仍保存在通知中心。' : '任务更新、审批和操作结果会显示在这里。'} action={unreadOnly ? <Button variant="outline" size="sm" onClick={() => setUnreadOnly(false)}>查看全部通知</Button> : undefined} />}
        {!loading && !items.length && error && <FoxEmptyState variant="error" title="通知同步失败" description={error} action={<Button variant="outline" size="sm" onClick={() => void load()}>重试</Button>} />}
        {priorityItems.length > 0 && <section className="fox-notification-day is-priority" aria-labelledby="notification-priority-title">
          <div className="fox-notification-day-title" id="notification-priority-title">需要处理</div>
          <div className="fox-notification-day-items">
            {priorityItems.map(renderNotificationItem)}
          </div>
        </section>}
        {notificationGroups.map((group) => <section className="fox-notification-day" key={group.key} aria-labelledby={`notification-day-${group.key}`}>
          <div className="fox-notification-day-title" id={`notification-day-${group.key}`}>{group.label}</div>
          <div className="fox-notification-day-items">
            {group.items.map(renderNotificationItem)}
          </div>
        </section>)}
      </div></ScrollArea>
    </section>}
    {createPortal(<ToastStack label="通知更新" position="top-right" visibleToasts={3} />, document.body)}
  </>
}

export function NotificationCenter(props: Parameters<typeof NotificationCenterContent>[0]) {
  return <ToastStackProvider duration={5000} limit={12}><NotificationCenterContent {...props} /></ToastStackProvider>
}
