import { inNotificationHost, type NotificationBridge, type NotificationCenterProps } from './notification-center'
import { createPortal } from 'react-dom'
import './notification-center.css'
import { ToastStack, ToastStackProvider, useToastStack, type ToastType } from '@/components/arc/toast-stack/toast-stack'
import { AnimatedIconTrigger, Bell as AnimatedBell } from '@/components/fox-ui/animated-icons'
import { FoxEmptyState } from '@/components/fox-ui/empty-state'
import { SkeletonContent } from '@/components/ui/skeleton'
import { Progress } from '@/components/ui/progress'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { desktopClient, desktopErrorDetails, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import { readDeveloperUi } from '@/features/chat/developer-ui'
import { userFacingError } from '@/features/chat/send-failure-notice'
import { type AppNotificationRecord, type NotificationPreferencesRecord } from '@/features/conversations/model/types'
import { FOX_IN_APP_NOTIFICATION_EVENT, FOX_NOTIFICATIONS_CHANGED_EVENT, FOX_NOTIFICATION_PREFERENCES_CHANGED_EVENT, type InAppNotificationEventDetail, directToastRecently, normalizeNotificationSoundId, notificationIsVisibleInContext, playNotificationSound, notify as toast } from '@/features/notifications'
import { type WorkspaceView } from '@/features/workspace/types'
import { ContactAvatar } from '@/features/companions/chat-controls'
import { useContacts } from '@/features/companions/context'
import { AlertTriangle, Bell, CheckCheck, Eraser, ExternalLink, FolderOpen, MoreHorizontal, RotateCcw, Bot, Database, ListTodo, MessageSquare, Settings, X, Check } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

type ToastNotification = Pick<AppNotificationRecord, 'id' | 'kind' | 'title' | 'body'> & Partial<AppNotificationRecord>

function noticeError(cause: unknown, fallback: string) {
  return userFacingError(desktopErrorDetails(cause).message, readDeveloperUi(), fallback)
}

/** 失败通知的正文在显示时再过一遍：普通模式不出现错误码、路径、密钥名；存档保留原文，开发者模式照常可见。 */
function displayBody(item: Pick<AppNotificationRecord, 'kind' | 'body'>): string {
  if (item.kind !== 'failed') return item.body
  return userFacingError(item.body, readDeveloperUi(), '这次没有完成，请重试。')
}
const toastType: Record<AppNotificationRecord['kind'], ToastType> = { completed: 'success', failed: 'error', progress: 'loading', approval: 'warning', question: 'info', message: 'info' }

/** K6: per-contact reminder mode gates active alerts (popup/sound/toast). The
 * center keeps the entry in every mode; only the interruption is suppressed.
 * Mirrors the backend `reminder_allows_alert`. */
function contactReminderAllowsAlert(item: AppNotificationRecord): boolean {
  const mode = item.action?.reminderMode
  if (mode === 'muted') return false
  if (mode === 'action_only') return item.kind === 'approval' || item.kind === 'question'
  return true
}

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
  if (typeof item.action?.coordinationConversationId === 'string') return { Icon: MessageSquare, label: '伙伴协作', action: '打开协作对话' }
  if (view.includes('knowledge') || view === 'local-files' || item.sourceType === 'knowledge') return { Icon: Database, label: '资料库', action: '打开资料库' }
  if (view === 'agents' || view === 'agent-detail') return { Icon: Bot, label: '专家', action: '查看专家' }
  if (item.sourceType === 'companion') return { Icon: MessageSquare, label: '伙伴', action: '打开私聊' }
  if (view === 'chat' || item.sourceType === 'run') return { Icon: MessageSquare, label: '对话', action: '打开对话' }
  if (view === 'model-service') return { Icon: Settings, label: '模型服务', action: '去设置模型服务' }
  if (view.startsWith('settings')) return { Icon: Settings, label: '设置', action: '去设置' }
  return { Icon: ListTodo, label: '任务', action: '查看任务' }
}

function NotificationCenterContent({
  onNavigate,
  open,
  onOpenChange,
  showTrigger,
  triggerHost,
  panelHost,
  activeView,
  activeEntityId,
  activeConversationId,
  bridge,
}: NotificationCenterProps & { bridge: NotificationBridge }) {
  const [filter, setFilter] = useState<'all' | 'unread' | 'action'>('all')
  const contactsContext = useContacts()
  const [items, setItems] = useState<AppNotificationRecord[]>([])
  const [preferences, setPreferences] = useState<NotificationPreferencesRecord | null>(null)
  const [loading, setLoading] = useState(desktopRuntimeAvailable)
  const [error, setError] = useState<string | null>(null)
  const { toast: showToast, dismiss: dismissToast } = useToastStack()
  const queuedFingerprints = useRef(new Map<string, string>())
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const observedNotifications = useRef<Map<string, string> | null>(null)
  const loadGeneration = useRef(0)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const closeRef = useRef<HTMLButtonElement>(null)
  useEffect(() => { if (open) closeRef.current?.focus() }, [open, panelHost])
  const closeCenter = () => {
    onOpenChange(false)
    triggerRef.current?.focus()
  }
  const openCenter = useRef(onOpenChange)
  openCenter.current = onOpenChange
  const navigateRef = useRef(onNavigate)
  navigateRef.current = onNavigate
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
      description: displayBody(notification),
      icon: notification.sourceType === 'companion' ? (() => { const companion = contactsContext?.list.companions.find(item => item.id === notification.sourceId); return companion ? <ContactAvatar contact={companion} /> : undefined })() : undefined,
      // Background progress should also leave the screen after a short wait; history stays in the center.
      duration: notification.kind === 'failed' || notification.kind === 'approval' ? 8000 : 5000,
      // UI 1103: a notice that points at a settings page (an external Agent
      // not set up, 模型服务) gets its own「去设置」button, straight there.
      action: notification.workspaceView && (notification.workspaceView.startsWith('settings') || notification.workspaceView === 'model-service') ? { label: notificationDestination({ ...notification, workspaceView: notification.workspaceView } as AppNotificationRecord).action, onClick: () => {
        navigateRef.current(notification.workspaceView as WorkspaceView, notification.entityId ?? undefined)
        if (notification.readAt == null && notification.sourceType) void desktopClient.setAppNotificationRead(notification.id, true)
          .then(() => window.dispatchEvent(new Event(FOX_NOTIFICATIONS_CHANGED_EVENT)))
          .catch(() => {})
      } } : notification.sourceType ? { label: '查看', onClick: () => {
        openCenter.current(true)
        setFilter('all')
        setExpandedId(notification.id)
        if (notification.readAt == null) void desktopClient.setAppNotificationRead(notification.id, true)
          .then(() => window.dispatchEvent(new Event(FOX_NOTIFICATIONS_CHANGED_EVENT)))
          .catch(cause => toast.error('标记通知已读失败', { description: noticeError(cause, '请重试。') }))
      } } : undefined,
    })
  }, [contactsContext?.list.companions, showToast])

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
        desktopClient.listAppNotifications(false, 100),
        desktopClient.getNotificationPreferences(),
      ])
      if (generation !== loadGeneration.current) return
      const visibleUnreadIds = new Set(nextItems
        .filter((item) => item.readAt == null && notificationIsVisibleInContext(item, {
          appVisible: document.visibilityState === 'visible',
          appFocused: document.hasFocus(),
          activeView: open ? 'notifications' : activeView,
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
        // A refusal the page already toasted directly (toastNow) is only recorded here.
        const shouldAlert = changed.filter((item) => !(nextPreferences.quietProgress && item.kind === 'progress') && !directToastRecently(item.mergeKey) && contactReminderAllowsAlert(item))
        shouldAlert.slice(0, 12).reverse().forEach((item) => enqueueToast(item))
        if (shouldAlert.length && nextPreferences.sound) playNotificationSound(normalizeNotificationSoundId(nextPreferences.soundId))
        if (shouldAlert.length && nextPreferences.systemPopup && typeof Notification !== 'undefined' && Notification.permission === 'granted') {
          const first = shouldAlert[0]
          new Notification(first.title, { body: shouldAlert.length > 1 ? `${displayBody(first)}（另有 ${shouldAlert.length - 1} 条更新）` : displayBody(first), tag: first.mergeKey })
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
      if (generation === loadGeneration.current) setError(noticeError(cause, '通知没有同步成功，请重试。'))
    } finally {
      if (generation === loadGeneration.current) setLoading(false)
    }
  }, [activeConversationId, activeEntityId, activeView, bridge, enqueueToast, open])

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
  const actionCount = items.filter(isAutomaticPriorityNotification).length
  const filters = [
    { id: 'all', label: '全部', count: items.length },
    { id: 'unread', label: '未读', count: unreadCount },
    { id: 'action', label: '需处理', count: actionCount },
  ] as const
  const notificationGroups = useMemo(() => {
    const groups = new Map<string, { key: string; label: string; items: AppNotificationRecord[] }>()
    for (const item of items) {
      if (filter === 'unread' && item.readAt != null) continue
      if (filter === 'action' && !isAutomaticPriorityNotification(item)) continue
      const day = notificationDay(item)
      const group = groups.get(day.key)
      if (group) group.items.push(item)
      else groups.set(day.key, { ...day, items: [item] })
    }
    return [...groups.values()]
  }, [items, filter])

  const markRead = async (item: AppNotificationRecord) => {
    try {
      if (item.readAt == null) await desktopClient.setAppNotificationRead(item.id, true)
      dismissToast(item.id)
      await load(true)
    } catch (cause) {
      toast.error('标记通知已读失败', { description: noticeError(cause, '请重试。') })
    }
  }
  const openItem = async (item: AppNotificationRecord) => {
    try {
      if (item.readAt == null) await desktopClient.setAppNotificationRead(item.id, true)
      dismissToast(item.id)
      const coordinationId = item.action?.coordinationConversationId
      if (typeof coordinationId === 'string') {
        onNavigate('companions', undefined, { conversationId: coordinationId })
      } else if (item.sourceType === 'companion') {
        const companion = contactsContext?.list.companions.find(contact => contact.id === item.sourceId && !contact.archivedAt && !contact.deletingAt)
        if (companion) {
          if (activeView === 'chat' && (companion.isDefault || companion.workShortcutOrder != null)) onNavigate('chat', undefined, { conversationId: companion.conversationId })
          else onNavigate('companions', companion.id)
        }
        else if (item.workspaceView) onNavigate(item.workspaceView as WorkspaceView, item.entityId ?? undefined)
      } else if (item.workspaceView === 'chat' && item.entityId) {
        onNavigate('chat', undefined, { conversationId: item.entityId })
      } else if (item.workspaceView) {
        onNavigate(item.workspaceView as WorkspaceView, item.entityId ?? undefined)
      }
      await load(true)
    } catch (cause) {
      toast.error('打开通知失败', { description: noticeError(cause, '请重试。') })
    }
  }
  const openNotificationFile = async (item: AppNotificationRecord, reveal: boolean) => {
    const path = typeof item.action.path === 'string' ? item.action.path : null
    if (!path) return
    try {
      await desktopClient.openDownloadedFile(path, reveal)
    } catch (cause) {
      toast.error(reveal ? '无法打开所在文件夹' : '无法打开文件', { description: noticeError(cause, '请重试。') })
    }
  }
  const markAllRead = async () => {
    try {
      await desktopClient.markAllAppNotificationsRead()
      clearToastQueue()
      await load(true)
    } catch (cause) {
      toast.error('全部标记已读失败', { description: noticeError(cause, '请重试。') })
    }
  }
  const clearRead = async () => {
    try {
      await desktopClient.clearReadAppNotifications()
      clearToastQueue()
      setExpandedId(null)
      await load(true)
    } catch (cause) {
      toast.error('清除已读通知失败', { description: noticeError(cause, '请重试。') })
    }
  }
  const renderNotificationItem = (item: AppNotificationRecord) => {
    const expanded = expandedId === item.id
    const { Icon, label, action } = notificationDestination(item)
    const companion = item.sourceType === 'companion' ? contactsContext?.list.companions.find(contact => contact.id === item.sourceId) : undefined
    // Run records store the conversation title, rather than an event title.
    const title = item.sourceType === 'run'
      ? item.kind === 'failed' ? '任务运行失败'
        : item.status === 'cancelled' ? '任务已取消'
          : item.kind === 'completed' ? '任务已完成'
            : item.kind === 'approval' ? '等待确认'
              : item.kind === 'question' ? '等待回答'
                : item.status === 'queued' ? '任务正在排队' : '任务正在执行'
      : companion?.displayName ?? item.title
    const body = displayBody(item)
    const summary = item.sourceType === 'run' ? [item.title, body].filter(Boolean).join(' · ')
      : companion && item.title !== body && item.title !== companion.displayName
        ? [item.title, body].filter(Boolean).join(' · ') : body
    const expandable = summary.length > 42 || summary.includes('\n')
    const status = item.kind === 'failed' ? '异常' : item.kind === 'approval' ? '待确认' : item.kind === 'question' ? '待回答' : null
    const downloadedFile = item.action.type === 'downloaded_file' && typeof item.action.path === 'string'
    const canNavigate = item.workspaceView || companion || typeof item.action.coordinationConversationId === 'string'
    return <article key={item.id} className={`fox-notification-row ${item.readAt == null ? 'is-unread' : ''}`}>
      <span className={`fox-notification-avatar is-${item.kind} ${companion ? 'is-companion' : ''}`} role="img" aria-label={`${label}通知，${item.readAt == null ? '未读' : '已读'}`}>
        {companion ? <ContactAvatar contact={companion} /> : <Icon size={18} aria-hidden="true" />}
        {item.readAt == null && <i aria-hidden="true" />}
      </span>
      <div className="fox-notification-summary">
        <div className="fox-notification-row-head">
          <strong>{title}</strong>
          {status && <span className={`fox-notification-status is-${item.kind}`}>{status}</span>}
          <time dateTime={new Date(item.updatedAt).toISOString()}>{new Date(item.updatedAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}</time>
        </div>
        {summary && <div className={`fox-notification-body ${expanded ? 'is-expanded' : ''}`}>
          <span>{summary}</span>
          {expandable && <button type="button" aria-expanded={expanded} onClick={() => setExpandedId(expanded ? null : item.id)}>{expanded ? '收起' : '展开'}</button>}
        </div>}
        {item.progress != null && item.kind === 'progress' && <Progress value={item.progress} aria-label={`${item.title}进度`} className="mt-1 h-1" />}
      </div>
      <div className="fox-notification-row-actions">
        {item.readAt == null && <Button variant="ghost" size="icon-sm" aria-label={`标记「${title}」为已读`} title="标记为已读" onClick={() => void markRead(item)}><Check /></Button>}
        {downloadedFile && item.action.canOpenDirectly === true && <Button variant="outline" size="sm" onClick={() => void openNotificationFile(item, false)}><ExternalLink />打开文件</Button>}
        {downloadedFile && <Button variant="outline" size="sm" onClick={() => void openNotificationFile(item, true)}><FolderOpen />打开所在文件夹</Button>}
        {canNavigate && <Button variant={item.kind === 'approval' ? 'default' : 'outline'} size="sm" onClick={() => void openItem(item)}>{action}</Button>}
      </div>
    </article>
  }
  const emptyTitle = filter === 'unread' ? '没有未读消息' : filter === 'action' ? '没有需处理的消息' : '暂无消息'
  return <>
    {showTrigger && inNotificationHost(<AnimatedIconTrigger><button ref={triggerRef} type="button" className={`fox-notification-trigger ${open ? 'is-active' : ''}`} aria-label={unreadCount ? `消息中心，${unreadCount} 条未读消息` : '消息中心'} aria-pressed={open} aria-controls={open ? 'fox-notification-page' : undefined} title="消息中心" onClick={() => open ? closeCenter() : onOpenChange(true)}>
        <AnimatedBell />{preferences?.badge !== false && unreadCount > 0 && <span aria-hidden="true" />}
      </button></AnimatedIconTrigger>, triggerHost ?? undefined)}
    {open && inNotificationHost(<section id="fox-notification-page" className="fox-notification-page" aria-labelledby="fox-notification-heading" onKeyDown={event => {
      if (event.key === 'Escape' && !event.defaultPrevented) {
        event.stopPropagation()
        closeCenter()
      }
    }}>
      <header className="fox-notification-page-header">
        <h1 id="fox-notification-heading">消息中心</h1>
        <Button ref={closeRef} variant="ghost" size="icon-sm" aria-label="关闭消息中心" title="关闭消息中心" onClick={closeCenter}><X /></Button>
      </header>
      <div className="fox-notification-page-content">
        <div className="fox-notification-toolbar">
          <div className="fox-notification-filters" role="group" aria-label="筛选消息">
            {filters.map(item => <Button key={item.id} variant={filter === item.id ? 'secondary' : 'ghost'} size="sm" aria-pressed={filter === item.id} onClick={() => setFilter(item.id)}>{item.label}<span>{item.count}</span></Button>)}
          </div>
          <div className="fox-notification-toolbar-actions">
            <Button variant="ghost" size="sm" disabled={!unreadCount} onClick={() => void markAllRead()}><CheckCheck />全部已读</Button>
            <Button variant="ghost" size="sm" disabled={!hasReadNotifications} onClick={() => void clearRead()}><Eraser />清除已读</Button>
            <DropdownMenu>
              <DropdownMenuTrigger asChild><Button variant="ghost" size="icon-sm" aria-label="管理消息"><MoreHorizontal /></Button></DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onSelect={() => void load()}><RotateCcw />刷新消息</DropdownMenuItem>
                <DropdownMenuItem onSelect={() => onNavigate('settings')}><Settings />通知设置</DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>
        <div className="fox-notification-stream" aria-live="polite">
          {error && items.length > 0 && <div className="fox-notification-error" role="alert"><AlertTriangle />{error}</div>}
          {loading && !items.length && <SkeletonContent label="正在同步消息" avatar lines={3} />}
          {!loading && !notificationGroups.length && !error && <FoxEmptyState title={emptyTitle} description={filter === 'all' ? '任务更新、审批和操作结果会显示在这里。' : undefined} action={filter !== 'all' ? <Button variant="outline" size="sm" onClick={() => setFilter('all')}>查看全部消息</Button> : undefined} />}
          {!loading && !items.length && error && <FoxEmptyState variant="error" title="消息同步失败" description={error} action={<Button variant="outline" size="sm" onClick={() => void load()}>重试</Button>} />}
          {notificationGroups.map(group => <section className="fox-notification-group" key={group.key} aria-labelledby={`notification-day-${group.key}`}>
            <h2 id={`notification-day-${group.key}`}>{group.label}</h2>
            {group.items.map(renderNotificationItem)}
          </section>)}
        </div>
      </div>
    </section>, panelHost)}
    {createPortal(<ToastStack label="通知更新" position="top-right" visibleToasts={3} />, document.body)}
  </>
}

export function NotificationCenter(props: Parameters<typeof NotificationCenterContent>[0]) {
  return <ToastStackProvider duration={5000} limit={12}><NotificationCenterContent {...props} /></ToastStackProvider>
}
