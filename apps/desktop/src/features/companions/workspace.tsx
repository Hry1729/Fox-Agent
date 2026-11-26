import { useEffect, useState } from 'react'
import { ChevronRight, Inbox } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { FoxEmptyState } from '@/components/fox-ui/empty-state'
import { RuntimeApprovalPrompt } from '@/features/chat/components/RuntimeApprovalPrompt'
import { accessModeLabel, conversationAccessMode } from '@/features/chat/access-mode'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { pendingRuntimeApprovals, pendingRuntimeQuestion } from '@/features/conversations/model/pending-interactions'
import type { AppNotificationRecord, ApprovalDecision, ConversationDetail } from '@/features/conversations/model/types'
import { FOX_NOTIFICATIONS_CHANGED_EVENT } from '@/features/notifications'
import type { CompanionWorkspaceView, NavigateWorkspace } from '../workspace/types'
import type { Companion } from './api'
import { ContactAvatar } from './chat-controls'
import { showContactError, useContacts } from './context'
import { CompanionOverview, NewCompanionButton } from './sidebar'
import { CompanionSchedules, type WorkspaceSchedules } from './tasks'

export default function CompanionWorkspace({ view, schedules, navigate }: {
  view: CompanionWorkspaceView; schedules: WorkspaceSchedules; navigate: NavigateWorkspace
}) {
  const ctx = useContacts()
  const companions = ctx?.list.companions.filter(item => !item.archivedAt && !item.deletingAt) ?? []
  return <div className="fox-companion-workspace">
    {view === 'companion-schedules' ? <CompanionSchedules companions={companions} schedules={schedules} navigate={navigate} />
      : view === 'companion-overview' ? <Overview companions={companions} navigate={navigate} />
        : <CompanionInbox companions={companions} schedules={schedules} navigate={navigate} />}
  </div>
}

function Overview({ companions, navigate }: { companions: Companion[]; navigate: NavigateWorkspace }) {
  const ids = JSON.stringify(companions.map(item => item.conversationId))
  const [permissions, setPermissions] = useState(new Map<string, string>())
  useEffect(() => {
    let active = true
    const conversationIds: string[] = JSON.parse(ids)
    void Promise.allSettled(conversationIds.map(async id => {
      const detail = await desktopClient.loadConversation(id)
      return [id, accessModeLabel(conversationAccessMode(detail.conversation))] as const
    })).then(results => {
      if (active) setPermissions(new Map(results.flatMap(result => result.status === 'fulfilled' ? [result.value] : [])))
    })
    return () => { active = false }
  }, [ids])
  return <CompanionOverview navigate={navigate} permissions={permissions} />
}

function CompanionInbox({ companions, schedules, navigate }: {
  companions: Companion[]; schedules: WorkspaceSchedules; navigate: NavigateWorkspace
}) {
  const ctx = useContacts()
  const [notices, setNotices] = useState<AppNotificationRecord[]>([])
  const [details, setDetails] = useState(new Map<string, ConversationDetail>())
  const [failedDetails, setFailedDetails] = useState<string[]>([])
  const [refresh, setRefresh] = useState(0)
  const [failed, setFailed] = useState(false)
  const pending = companions.filter(item => item.pendingActionCount > 0)
  const activeIds = new Set(pending.map(item => item.id))
  const targets = JSON.stringify([...new Set([
    ...pending.map(item => item.conversationId),
    ...schedules.entries.filter(item => activeIds.has(item.companionId)).map(item => item.task.conversationId),
    ...notices.flatMap(item => item.sourceId && activeIds.has(item.sourceId) && typeof item.action.coordinationConversationId === 'string'
      ? [item.action.coordinationConversationId] : []),
  ])].sort())
  const activity = JSON.stringify(companions.map(item => [item.id, item.pendingActionCount, item.activeRunStatus, item.unreadCount]))
  useEffect(() => {
    const update = () => setRefresh(value => value + 1)
    window.addEventListener(FOX_NOTIFICATIONS_CHANGED_EVENT, update)
    return () => window.removeEventListener(FOX_NOTIFICATIONS_CHANGED_EVENT, update)
  }, [])
  useEffect(() => {
    if (!ctx) return
    let active = true
    void desktopClient.listAppNotifications(false, 100).then(items => {
      if (active) { setNotices(items); setFailed(false) }
    }).catch(() => { if (active) setFailed(true) })
    return () => { active = false }
  }, [Boolean(ctx), refresh, activity])
  useEffect(() => {
    let active = true
    const ids: string[] = JSON.parse(targets)
    void Promise.allSettled(ids.map(async id => [id, await desktopClient.loadConversation(id)] as const)).then(results => {
      if (!active) return
      setDetails(new Map(results.flatMap(result => result.status === 'fulfilled' ? [result.value] : [])))
      setFailedDetails(ids.filter((_, index) => results[index].status === 'rejected'))
    })
    return () => { active = false }
  }, [targets, refresh, activity])
  const open = (contact: Companion, conversationId = contact.conversationId) => navigate('companions', undefined, { conversationId })
  const running = companions.filter(item => item.activeRunStatus && item.activeRunStatus !== 'awaiting_confirmation')
  const recent = notices.flatMap(item => {
    const contact = companions.find(companion => companion.id === item.sourceId)
    return contact && item.sourceType === 'companion' && (item.kind === 'completed' || item.kind === 'failed' || ((item.kind === 'approval' || item.kind === 'question') && item.status === 'resolved'))
      ? [{ item, contact }] : []
  }).slice(0, 12)
  const resolve = async (detail: ConversationDetail, approvalId: string, decision: ApprovalDecision) => {
    try {
      if (!await desktopClient.resolveApproval(approvalId, decision, detail.conversation.agentBackend)) throw new Error('该审批已处理或失效。')
      setDetails(current => {
        const next = new Map(current)
        const saved = next.get(detail.conversation.id)
        if (saved) next.set(detail.conversation.id, { ...saved, approvals: saved.approvals.filter(item => item.id !== approvalId) })
        return next
      })
      await ctx?.refresh()
      setRefresh(value => value + 1)
      return true
    } catch (error) { showContactError(error); return false }
  }
  return <div className="fox-companion-workspace-inner fox-companion-inbox">
    <header className="fox-companion-workspace-head">
      <div><h1>待我处理</h1><p>查看伙伴等待确认的操作、后台任务和近期交付。</p></div>
      <NewCompanionButton />
    </header>
    <section className="fox-companion-inbox-group">
      <h2>等待你确认 <small>{pending.reduce((total, item) => total + item.pendingActionCount, 0)} 项</small></h2>
      {failedDetails.length > 0 && <div role="alert">部分待确认事项未能加载。<Button size="sm" variant="ghost" onClick={() => setRefresh(value => value + 1)}>重试</Button></div>}
      {!pending.length && <FoxEmptyState icon={<Inbox />} title="暂无待确认事项" description="伙伴需要你批准操作或补充信息时，会显示在这里。" />}
      {pending.map(contact => {
        const conversationIds = new Set([
          contact.conversationId,
          ...schedules.entries.filter(item => item.companionId === contact.id).map(item => item.task.conversationId),
          ...notices.flatMap(item => item.sourceId === contact.id && typeof item.action.coordinationConversationId === 'string' ? [item.action.coordinationConversationId] : []),
        ])
        const ownedDetails = [...conversationIds].flatMap(id => details.get(id) ?? [])
        return <article className="fox-companion-inbox-card" key={contact.id}>
          <div className="fox-companion-workspace-row">
            <div className="fox-companion-workspace-person"><ContactAvatar contact={contact} /><span><strong>{contact.displayName}</strong><small>{contact.pendingActionCount} 项待处理</small></span></div>
            <Button size="sm" variant="ghost" onClick={() => open(contact)}>进入私聊<ChevronRight /></Button>
          </div>
          {ownedDetails.map(detail => <div key={detail.conversation.id} className="fox-companion-inbox-decisions">
            {pendingRuntimeApprovals(detail).map(approval => <RuntimeApprovalPrompt key={approval.id} approval={approval} contactStyle
              onResolve={(id, choice) => resolve(detail, id, choice)} />)}
            {pendingRuntimeQuestion(detail) && <Button size="sm" variant="outline" onClick={() => open(contact, detail.conversation.id)}>打开对话回答问题</Button>}
          </div>)}
        </article>
      })}
    </section>
    {running.length > 0 && <section className="fox-companion-inbox-group">
      <h2>正在后台执行 <small>{running.length} 位</small></h2>
      {running.map(contact => <article className="fox-companion-workspace-row fox-companion-inbox-card" key={contact.id}>
        <div className="fox-companion-workspace-person"><ContactAvatar contact={contact} /><span><strong>{contact.displayName}</strong><small>{contact.activeRunStatus === 'queued' ? '排队中' : contact.activeRunStatus === 'cancelling' ? '正在取消' : '执行中'}</small></span></div>
        <Button size="sm" variant="ghost" onClick={() => open(contact)}>进入私聊<ChevronRight /></Button>
      </article>)}
    </section>}
    {(recent.length > 0 || failed) && <section className="fox-companion-inbox-group">
      <h2>近期交付与已处理 <small>{recent.length} 项</small></h2>
      {failed && <div role="alert">未能加载近期记录。<Button size="sm" variant="ghost" onClick={() => setRefresh(value => value + 1)}>重试</Button></div>}
      {recent.map(({ item, contact }) => <article className="fox-companion-workspace-row fox-companion-inbox-card" key={item.id}>
        <div className="fox-companion-workspace-person"><ContactAvatar contact={contact} /><span><strong>{item.title}</strong><small>{contact.displayName} · {new Date(item.updatedAt).toLocaleString('zh-CN')}</small>{item.kind === 'completed' && item.body && <small>{item.body}</small>}</span></div>
        <Button size="sm" variant="ghost" onClick={() => open(contact, typeof item.action.coordinationConversationId === 'string' ? item.action.coordinationConversationId : undefined)}>查看对话<ChevronRight /></Button>
      </article>)}
    </section>}
  </div>
}
