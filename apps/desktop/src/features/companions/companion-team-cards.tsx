// Adapted from OpenMausBot src/components/ApprovalCard.tsx at 1a4e4da9f380804ad98c7fd1b141bb0dbfafced4 (Apache-2.0).
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from 'react'
import { listen } from '@tauri-apps/api/event'
import { Check, Undo2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import { COMPANION_TEAM_ACTIONS_CHANGED, contacts, type CompanionTeamAction, type CompanionTeamDecision, type CompanionTeamValues } from './api'
import { ContactChoice, ContactDecision } from './chat-decision'
import { showContactError, useContacts } from './context'
import { ContactDeletePlan } from './delete'

const TeamActionsContext = createContext<{
  actions: CompanionTeamAction[]
  resolve: (id: string, decision: CompanionTeamDecision) => Promise<void>
} | null>(null)

export function CompanionTeamActionsProvider({ children }: { children: ReactNode }) {
  const context = useContacts()
  if (!context?.privateChat || !context.current?.isDefault) return children
  return <TeamActionSession key={context.current.id} companionId={context.current.id} refreshContacts={context.refresh}>{children}</TeamActionSession>
}

function TeamActionSession({ companionId, refreshContacts, children }: { companionId: string; refreshContacts: () => Promise<void>; children: ReactNode }) {
  const [actions, setActions] = useState<CompanionTeamAction[]>([])
  const [error, setError] = useState('')
  const revision = useRef(0)
  const reload = useCallback(async () => {
    const ticket = ++revision.current
    try {
      const records = await contacts.teamActions(companionId)
      if (ticket === revision.current) {
        setActions(records)
        setError('')
      }
    } catch (cause) {
      if (ticket === revision.current) setError(desktopErrorDetails(cause).message)
    }
  }, [companionId])

  useEffect(() => {
    let active = true
    const subscription = listen<{ companionId: string }>(COMPANION_TEAM_ACTIONS_CHANGED, ({ payload }) => {
      if (!active || payload.companionId !== companionId) return
      void reload()
      void refreshContacts().catch(showContactError)
    })
    void subscription.catch(() => {}).then(() => { if (active) void reload() })
    return () => {
      active = false
      ++revision.current
      void subscription.then(stop => stop()).catch(() => {})
    }
  }, [companionId, refreshContacts, reload])

  const resolve = async (id: string, decision: CompanionTeamDecision) => {
    const updated = await contacts.resolveTeamAction(id, decision)
    ++revision.current
    setActions(records => records.map(record => record.id === updated.id ? updated : record))
    void reload()
    void refreshContacts().catch(showContactError)
  }
  return <TeamActionsContext.Provider value={{ actions, resolve }}>
    {children}
    {error && <div className="fox-contact-decision-entry">
      <p role="alert" className="fox-contact-decision-error">操作记录加载失败。{error}</p>
      <Button type="button" size="sm" variant="ghost" onClick={() => void reload()}>重新加载</Button>
    </div>}
  </TeamActionsContext.Provider>
}

export function useCompanionTeamActions() {
  return useContext(TeamActionsContext)?.actions
}

export function CompanionTeamCards({ runId }: { runId: string }) {
  const team = useContext(TeamActionsContext)
  if (!team) return null
  return <>{team.actions.filter(action => action.runId === runId).map(action =>
    <div key={action.id} className="fox-contact-decision-entry">
      <CompanionTeamCard action={action} onResolve={decision => team.resolve(action.id, decision)} />
    </div>)}</>
}

const fieldLabels = {
  name: '名称', duty: '职责', instructions: '工作要求', providerId: '模型服务', modelRef: '模型',
} satisfies Record<keyof CompanionTeamValues, string>

const appliedTitle = { create: '已新建', profile: '已修改资料', model: '已更换模型' }
const operation = { archive: '归档', delete: '删除' }

function CompanionTeamCard({ action, onResolve }: { action: CompanionTeamAction; onResolve: (decision: CompanionTeamDecision) => Promise<void> }) {
  const [busy, setBusy] = useState<CompanionTeamDecision | null>(null)
  const [error, setError] = useState<{ code: string; message: string }>()
  const resolve = async (decision: CompanionTeamDecision) => {
    setBusy(decision)
    setError(undefined)
    try {
      await onResolve(decision)
    } catch (cause) {
      setError(desktopErrorDetails(cause))
    } finally {
      setBusy(null)
    }
  }
  const name = `「${action.targetName}」`
  const stale = error?.code === 'companion.team_action_stale'
  const errorMessage = error && ({
    'companion.run_active': '伙伴还在运行。等它停下后再试。',
    'companion.plan_changed': '删除清单已变化。请取消这条请求，再请小狐重新生成清单。',
    'companion.team_action_stale': action.status === 'pending' ? '伙伴状态已变化，无法执行这次操作。' : '伙伴状态已变化，无法撤销这次操作。',
  }[error.code] ?? error.message)
  const errorNote = errorMessage && <p role="alert" className="fox-contact-decision-error">{errorMessage}</p>

  if (action.kind === 'delete' || action.kind === 'archive') {
    const verb = operation[action.kind]
    const preview = action.preview
    const deletion = action.kind === 'delete'
    const pending = action.status === 'pending'
    return <ContactDecision
      resolved={!pending}
      title={`${pending ? `要${verb}` : action.status === 'confirmed' ? `已${verb}` : `已取消${verb}`}${name}${pending ? '吗？' : ''}`}
      description={pending ? (deletion ? '删除后无法撤销。' : '可以从已归档列表恢复这个伙伴。') : undefined}
    >
      {pending && preview && <ContactDeletePlan plan={preview.plan} planHash={preview.planHash} />}
      {pending ? <>
        {deletion && !preview && <p role="alert">缺少删除清单。请取消这条请求，再请小狐重新生成清单。</p>}
        <div className="fox-contact-choices">
          <ContactChoice index={0} disabled={busy !== null || stale || (deletion && (!preview || error?.code === 'companion.plan_changed'))} onClick={() => void resolve('confirm')}>
            {busy === 'confirm' ? `正在${verb}…` : deletion ? '永久删除' : '确认归档'}
          </ContactChoice>
          <ContactChoice index={1} disabled={busy !== null} onClick={() => void resolve('dismiss')}>{busy === 'dismiss' ? '正在取消…' : '取消'}</ContactChoice>
        </div>
      </> : <p role="status" className="fox-contact-receipt">
        {action.status === 'confirmed' ? <><Check size={15} aria-hidden="true" />已确认{verb}</> : '已取消'}
      </p>}
      {errorNote}
    </ContactDecision>
  }

  const before = action.before ?? {}
  const after = action.after ?? {}
  const creating = action.kind === 'create'
  const fields = (Object.keys(fieldLabels) as Array<keyof CompanionTeamValues>)
    .filter(key => creating ? after[key] != null : before[key] !== after[key])
  const undone = action.status === 'undone'
  const value = (key: keyof CompanionTeamValues, values: CompanionTeamValues) =>
    values[key] || (key === 'providerId' || key === 'modelRef' ? '默认模型' : '未设置')
  return <ContactDecision title={`${appliedTitle[action.kind]}${name}`}>
    {fields.length > 0 && <pre className="fox-contact-operation" tabIndex={0} aria-label={creating ? '新伙伴资料' : '修改前后的值'}>
      {fields.map(key => creating
        ? `${fieldLabels[key]}  ${value(key, after)}`
        : `${fieldLabels[key]}\n改前  ${value(key, before)}\n改后  ${value(key, after)}`).join(creating ? '\n' : '\n\n')}
    </pre>}
    {undone && <p role="status" className="fox-contact-receipt"><Undo2 size={15} aria-hidden="true" />已撤销</p>}
    {action.status === 'applied' && <Button type="button" variant="ghost" size="sm" disabled={busy !== null || stale} onClick={() => void resolve('undo')}>
      {busy === 'undo' ? '正在撤销…' : '撤销'}
    </Button>}
    {errorNote}
  </ContactDecision>
}
