import { useEffect, useMemo, useState } from 'react'
import { Bot, ChevronRight, CircleStop, Clock3, Copy, FileText, KeyRound, LoaderCircle, Pause, Play, Plus, RefreshCw, RotateCcw, Send, ShieldCheck, Webhook } from 'lucide-react'
import { notify as toast } from '@/features/notifications'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Textarea } from '@/components/ui/textarea'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type {
  AgentRecord,
  DigitalColleagueAuditRecord,
  DigitalColleagueChannelRecord,
  DigitalColleagueRecord,
  DigitalColleagueScheduleRecord,
  DigitalColleagueTriggerRecord,
  ConversationDetail,
  KnowledgeBaseRecord,
  ProjectRecord,
} from '@/features/conversations/model/types'
import { remoteKnowledgeReference } from './expert-knowledge'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  experts: AgentRecord[]
}

const statusLabel: Record<DigitalColleagueRecord['status'], string> = {
  active: '运行中',
  paused: '已暂停',
  revoked: '已撤销',
}

const triggerStatusLabel: Record<DigitalColleagueTriggerRecord['status'], string> = {
  accepted: '已接收', queued: '排队中', running: '执行中', completed: '已完成', failed: '失败',
  cancelled: '已取消', rejected: '已拒绝', skipped: '已跳过',
}

function formatTime(value: number | null) {
  return value ? new Date(value).toLocaleString('zh-CN') : '—'
}

function formatTriggerDuration(trigger: DigitalColleagueTriggerRecord) {
  if (!trigger.completedAt) return ['accepted', 'queued', 'running'].includes(trigger.status) ? '进行中' : '—'
  const duration = Math.max(0, trigger.completedAt - trigger.createdAt)
  if (duration < 1000) return `${duration} ms`
  if (duration < 60_000) return `${(duration / 1000).toFixed(1)} s`
  return `${Math.round(duration / 60_000)} min`
}

export function DigitalColleagueManager({ open, onOpenChange, experts }: Props) {
  const availableExperts = useMemo(() => experts.filter((expert) => expert.runtimeType === 'pi' && expert.available), [experts])
  const [tab, setTab] = useState<'manage' | 'create'>('manage')
  const [busy, setBusy] = useState(false)
  const [colleagues, setColleagues] = useState<DigitalColleagueRecord[]>([])
  const [selectedId, setSelectedId] = useState('')
  const [projects, setProjects] = useState<ProjectRecord[]>([])
  const [knowledgeBases, setKnowledgeBases] = useState<KnowledgeBaseRecord[]>([])
  const [schedules, setSchedules] = useState<DigitalColleagueScheduleRecord[]>([])
  const [channels, setChannels] = useState<DigitalColleagueChannelRecord[]>([])
  const [triggers, setTriggers] = useState<DigitalColleagueTriggerRecord[]>([])
  const [audit, setAudit] = useState<DigitalColleagueAuditRecord[]>([])
  const [selectedTriggerId, setSelectedTriggerId] = useState('')
  const [runDetail, setRunDetail] = useState<ConversationDetail | null>(null)
  const [runDetailBusy, setRunDetailBusy] = useState(false)
  const [runDetailError, setRunDetailError] = useState<string | null>(null)
  const [oneTimeSecret, setOneTimeSecret] = useState('')
  const [name, setName] = useState('')
  const [expertId, setExpertId] = useState('')
  const [objective, setObjective] = useState('')
  const [projectId, setProjectId] = useState('none')
  const [knowledgeIds, setKnowledgeIds] = useState<Set<string>>(new Set())
  const [runsPerDay, setRunsPerDay] = useState('8')
  const [tokensPerDay, setTokensPerDay] = useState('50000')
  const [durationMinutes, setDurationMinutes] = useState('15')
  const [toolCalls, setToolCalls] = useState('30')
  const [scheduleName, setScheduleName] = useState('定期巡检')
  const [intervalMinutes, setIntervalMinutes] = useState('60')
  const [manualPayload, setManualPayload] = useState('{}')
  const [channelName, setChannelName] = useState('Webhook')
  const [channelIdentity, setChannelIdentity] = useState('')

  const selected = colleagues.find((item) => item.id === selectedId) ?? null

  const loadDetails = async (colleagueId: string) => {
    const [nextSchedules, nextChannels, nextTriggers, nextAudit] = await Promise.all([
      desktopClient.listDigitalColleagueSchedules(colleagueId),
      desktopClient.listDigitalColleagueChannels(colleagueId),
      desktopClient.listDigitalColleagueTriggers(colleagueId),
      desktopClient.listDigitalColleagueAudit(colleagueId),
    ])
    setSchedules(nextSchedules)
    setChannels(nextChannels)
    setTriggers(nextTriggers)
    setSelectedTriggerId((current) => nextTriggers.some((item) => item.id === current) ? current : nextTriggers[0]?.id ?? '')
    setAudit(nextAudit)
  }

  const selectedTrigger = triggers.find((item) => item.id === selectedTriggerId) ?? triggers[0] ?? null
  useEffect(() => {
    if (!selectedTrigger?.runId) {
      setRunDetail(null)
      setRunDetailError(null)
      return
    }
    let cancelled = false
    setRunDetailBusy(true)
    setRunDetailError(null)
    void desktopClient.getDigitalColleagueRunDetail(selectedTrigger.runId).then((value) => {
      if (!cancelled) setRunDetail(value)
    }).catch((cause) => {
      if (!cancelled) setRunDetailError(cause instanceof Error ? cause.message : String(cause))
    }).finally(() => { if (!cancelled) setRunDetailBusy(false) })
    return () => { cancelled = true }
  }, [selectedTrigger?.runId])

  const refresh = async (preferredId?: string) => {
    const records = await desktopClient.listDigitalColleagues()
    setColleagues(records)
    const nextId = preferredId && records.some((item) => item.id === preferredId)
      ? preferredId
      : records.some((item) => item.id === selectedId) ? selectedId : records[0]?.id ?? ''
    setSelectedId(nextId)
    if (nextId) await loadDetails(nextId)
    else {
      setSchedules([]); setChannels([]); setTriggers([]); setAudit([])
    }
  }

  useEffect(() => {
    if (!open || !desktopRuntimeAvailable) return
    let active = true
    setBusy(true)
    void Promise.all([
      desktopClient.listDigitalColleagues(),
      desktopClient.listProjects(),
      desktopClient.listKnowledgeBases().catch(() => [] as KnowledgeBaseRecord[]),
    ]).then(async ([records, projectRecords, bases]) => {
      if (!active) return
      setColleagues(records)
      setProjects(projectRecords.filter((project) => project.status === 'active'))
      setKnowledgeBases(bases)
      setExpertId((value) => value || availableExperts[0]?.id || '')
      const nextId = records[0]?.id ?? ''
      setSelectedId(nextId)
      if (nextId) await loadDetails(nextId)
    }).catch((cause) => {
      if (active) toast.error('读取数字同事失败', { description: cause instanceof Error ? cause.message : String(cause) })
    }).finally(() => { if (active) setBusy(false) })
    return () => { active = false }
  }, [open, availableExperts])

  const runAction = async (action: () => Promise<unknown>, success: string, colleagueId = selectedId) => {
    setBusy(true)
    try {
      await action()
      await refresh(colleagueId)
      toast.success(success)
    } catch (cause) {
      toast.error('操作失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally {
      setBusy(false)
    }
  }

  const createColleague = async () => {
    const expert = availableExperts.find((item) => item.id === expertId)
    if (!name.trim() || !objective.trim() || !expert) {
      toast.error('请选择专家并填写名称与长期目标')
      return
    }
    const project = projects.find((item) => item.id === projectId)
    setBusy(true)
    try {
      const record = await desktopClient.createDigitalColleague({
        name: name.trim(), expertId: expert.id, objective: objective.trim(),
        projectId: project?.id ?? null, projectRoot: project?.rootPath ?? null,
        knowledgeReferences: [...knowledgeIds].map((id) => remoteKnowledgeReference(id)),
        maxRunsPerDay: Number(runsPerDay), maxTokensPerDay: Number(tokensPerDay),
        maxDurationMs: Number(durationMinutes) * 60_000, maxOutputTokens: 4_000,
        maxToolCalls: Number(toolCalls),
      })
      await refresh(record.id)
      setTab('manage')
      setName(''); setObjective(''); setKnowledgeIds(new Set())
      toast.success('数字同事已创建，并冻结当前专家版本')
    } catch (cause) {
      toast.error('创建数字同事失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally {
      setBusy(false)
    }
  }

  const triggerManual = async () => {
    if (!selected) return
    let payload: unknown
    try { payload = JSON.parse(manualPayload) } catch { toast.error('触发数据必须是有效 JSON'); return }
    await runAction(() => desktopClient.triggerDigitalColleague(selected.id, payload), '任务已进入隔离执行队列')
  }

  const createChannel = async () => {
    if (!selected || !channelIdentity.trim()) return
    setBusy(true)
    try {
      const created = await desktopClient.createDigitalColleagueChannel({
        colleagueId: selected.id, name: channelName.trim() || 'Webhook', channelKind: 'webhook',
        externalIdentity: channelIdentity.trim(), rateLimitPerMinute: 30,
      })
      setOneTimeSecret(created.secret)
      setChannelIdentity('')
      await refresh(selected.id)
      toast.success('签名 Channel 已创建；密钥只显示这一次')
    } catch (cause) {
      toast.error('创建 Channel 失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally { setBusy(false) }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="fox-agent-editor-dialog">
        <DialogHeader>
          <DialogTitle>数字同事</DialogTitle>
          <DialogDescription>把已版本化专家部署为可治理的长期实体；项目、知识、配额、调度、签名入口和每次运行都有持久记录。</DialogDescription>
        </DialogHeader>
        <Tabs value={tab} onValueChange={(value) => setTab(value as typeof tab)}>
          <TabsList><TabsTrigger value="manage">管理（{colleagues.length}）</TabsTrigger><TabsTrigger value="create">新建</TabsTrigger></TabsList>
          <TabsContent value="manage" className="fox-agent-editor-resources">
            <section>
              <header><span><Bot />数字同事</span><Button variant="ghost" size="sm" disabled={busy} onClick={() => void refresh()}><RefreshCw className={busy ? 'animate-spin' : ''} />刷新</Button></header>
              {colleagues.length ? <Select value={selectedId} onValueChange={(value) => { setSelectedId(value); setOneTimeSecret(''); void loadDetails(value) }}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent>{colleagues.map((item) => <SelectItem key={item.id} value={item.id}>{item.name} · {statusLabel[item.status]}</SelectItem>)}</SelectContent></Select> : <p className="fox-agent-resource-empty">还没有数字同事。切换到“新建”部署第一个长期实体。</p>}
            </section>
            {selected && <>
              <Card className="fox-agent-work-card fox-agent-section-card">
                <div className="fox-agent-work-head"><h2>{selected.name}</h2><Badge variant={selected.status === 'active' ? 'secondary' : 'outline'}>{statusLabel[selected.status]}</Badge></div>
                <p>{selected.objective}</p>
                <small>专家 {experts.find((item) => item.id === selected.expertId)?.name ?? selected.expertId} · SHA-256 {selected.packageHash.slice(0, 12)}… · {selected.maxRunsPerDay} 次/日 · {selected.maxTokensPerDay.toLocaleString()} tokens/日</small>
                <div className="fox-agent-editor-actions">
                  {selected.status !== 'revoked' && <Button variant="outline" size="sm" disabled={busy} onClick={() => void runAction(() => desktopClient.setDigitalColleaguePaused(selected.id, selected.status === 'active'), selected.status === 'active' ? '数字同事已暂停；调度已停用' : '数字同事已恢复')}>{selected.status === 'active' ? <Pause /> : <Play />}{selected.status === 'active' ? '暂停' : '恢复'}</Button>}
                  {selected.status !== 'revoked' && <Button variant="destructive" size="sm" disabled={busy} onClick={() => { if (window.confirm(`撤销“${selected.name}”及其全部外部入口？此操作不可恢复。`)) void runAction(() => desktopClient.revokeDigitalColleague(selected.id, 'revoked by user'), '数字同事与外部入口已撤销') }}>撤销</Button>}
                </div>
              </Card>
              <section><header><span><Send />手动触发</span><b>外部数据按低权限上下文处理</b></header><Textarea value={manualPayload} onChange={(event) => setManualPayload(event.target.value)} /><Button size="sm" disabled={busy || selected.status !== 'active'} onClick={() => void triggerManual()}><Send />运行一次</Button></section>
              <section><header><span><Clock3 />持久调度</span><b>{schedules.length} 条</b></header><div className="fox-agent-editor-profile-row"><Input value={scheduleName} onChange={(event) => setScheduleName(event.target.value)} placeholder="调度名称" /><Input type="number" min="1" value={intervalMinutes} onChange={(event) => setIntervalMinutes(event.target.value)} aria-label="间隔分钟" /><Button size="sm" disabled={busy || selected.status !== 'active'} onClick={() => void runAction(() => desktopClient.saveDigitalColleagueSchedule({ colleagueId: selected.id, name: scheduleName, intervalSeconds: Number(intervalMinutes) * 60, catchupWindowSeconds: 300, enabled: true }), '调度已保存')}><Plus />添加</Button></div><div className="fox-agent-resource-list">{schedules.map((item) => <div key={item.id}><span><Clock3 /></span><p><b>{item.name}</b><small>每 {Math.round(item.intervalSeconds / 60)} 分钟 · 下次 {formatTime(item.nextDueAt)}</small></p><Switch checked={item.enabled} disabled={busy || selected.status !== 'active'} onCheckedChange={(enabled) => void runAction(() => desktopClient.saveDigitalColleagueSchedule({ scheduleId: item.id, colleagueId: selected.id, name: item.name, intervalSeconds: item.intervalSeconds, catchupWindowSeconds: item.catchupWindowSeconds, enabled }), enabled ? '调度已启用' : '调度已停用')} /></div>)}</div></section>
              <section><header><span><Webhook />签名 Channel</span><b>{channels.filter((item) => item.status === 'active').length} 个活动入口</b></header><div className="fox-agent-editor-profile-row"><Input value={channelName} onChange={(event) => setChannelName(event.target.value)} placeholder="入口名称" /><Input value={channelIdentity} onChange={(event) => setChannelIdentity(event.target.value)} placeholder="精确发送方身份" /><Button size="sm" disabled={busy || selected.status !== 'active' || !channelIdentity.trim()} onClick={() => void createChannel()}><KeyRound />创建</Button></div>{oneTimeSecret && <Card className="fox-agent-section-card"><p><ShieldCheck />一次性签名密钥</p><code>{oneTimeSecret}</code><Button variant="outline" size="sm" onClick={() => void navigator.clipboard.writeText(oneTimeSecret).then(() => toast.success('密钥已复制'))}><Copy />复制</Button></Card>}<div className="fox-agent-resource-list">{channels.map((item) => <div key={item.id}><span><Webhook /></span><p><b>{item.name}<em>{item.status}</em></b><small>{item.externalIdentity} · {item.secretPrefix}… · {item.rateLimitPerMinute}/分钟</small></p>{item.status === 'active' && <Button variant="outline" size="sm" disabled={busy} onClick={() => void runAction(() => desktopClient.revokeDigitalColleagueChannel(item.id), 'Channel 已撤销')} >撤销</Button>}</div>)}</div></section>
              <section className="fox-colleague-run-section">
                <header><span><RefreshCw />运行与审计</span><b>{triggers.length} 次触发</b></header>
                <div className="fox-colleague-run-layout">
                  <div className="fox-agent-resource-list fox-colleague-run-list">
                    {triggers.slice(0, 24).map((item) => <button type="button" key={item.id} className={selectedTrigger?.id === item.id ? 'is-active' : ''} onClick={() => setSelectedTriggerId(item.id)}><span><Bot /></span><p><b>{item.sourceType} · {triggerStatusLabel[item.status]}</b><small>{formatTime(item.createdAt)} · {formatTriggerDuration(item)} · {item.totalTokens} tokens</small></p><ChevronRight /></button>)}
                    {!triggers.length && <p className="fox-agent-resource-empty">暂无运行记录。</p>}
                  </div>
                  {selectedTrigger && <Card className="fox-colleague-run-detail">
                    <div className="fox-agent-work-head"><h2>{triggerStatusLabel[selectedTrigger.status]}</h2><Badge variant={selectedTrigger.status === 'failed' ? 'destructive' : 'secondary'}>{selectedTrigger.sourceType}</Badge></div>
                    <div className="fox-colleague-run-facts"><span><small>触发时间</small><b>{formatTime(selectedTrigger.createdAt)}</b></span><span><small>执行时长</small><b>{formatTriggerDuration(selectedTrigger)}</b></span><span><small>工具</small><b>{selectedTrigger.toolCallCount}</b></span><span><small>Token</small><b>{selectedTrigger.totalTokens.toLocaleString()}</b></span></div>
                    {selectedTrigger.errorMessage && <p className="fox-setting-error">{selectedTrigger.errorMessage}</p>}
                    {runDetailBusy ? <div className="fox-colleague-run-state"><LoaderCircle className="animate-spin" />正在读取运行详情</div> : runDetailError ? <div className="fox-colleague-run-state is-error">{runDetailError}</div> : runDetail && <>
                      <div className="fox-colleague-run-summary"><span><Bot />子 Agent <b>{runDetail.childRuns.length}</b></span><span><FileText />产物 <b>{runDetail.artifacts.length}</b></span><span><ShieldCheck />证据 <b>{runDetail.evidence.length}</b></span></div>
                      {runDetail.artifacts.length > 0 && <div className="fox-colleague-run-artifacts">{runDetail.artifacts.slice(0, 8).map((artifact) => <div key={artifact.id}><FileText /><span><b>{artifact.displayName}</b><small>{artifact.storagePath}</small></span></div>)}</div>}
                    </>}
                    <p className="fox-colleague-adoption">采用状态：独立调度任务没有上层任务；结果保存在本次运行记录中。</p>
                    <div className="fox-agent-editor-actions">
                      {selectedTrigger.runId && ['accepted', 'queued', 'running'].includes(selectedTrigger.status) && <Button variant="outline" size="sm" disabled={busy} onClick={() => void runAction(() => desktopClient.cancelRun(selectedTrigger.runId!), '已发送停止信号')}><CircleStop />停止</Button>}
                      {!['accepted', 'queued', 'running'].includes(selectedTrigger.status) && <Button variant="outline" size="sm" disabled={busy} onClick={() => void runAction(() => desktopClient.triggerDigitalColleague(selected!.id, selectedTrigger.payload), '已按原始输入创建重试任务')}><RotateCcw />重试</Button>}
                    </div>
                  </Card>}
                </div>
                <details><summary>审计日志（{audit.length}）</summary><div className="fox-agent-resource-list">{audit.slice(0, 30).map((item) => <div key={item.id}><span><ShieldCheck /></span><p><b>{item.event} · {item.outcome}</b><small>{formatTime(item.createdAt)} · {item.actor}</small></p></div>)}</div></details>
              </section>
            </>}
          </TabsContent>
          <TabsContent value="create" className="fox-agent-editor-resources">
            <section><header><span><Bot />身份与目标</span><b>冻结专家当前版本</b></header><div className="fox-agent-editor-grid"><label><span>名称</span><Input value={name} onChange={(event) => setName(event.target.value)} placeholder="例如：发布质量守门员" /></label><label><span>基础专家</span><Select value={expertId} onValueChange={setExpertId}><SelectTrigger><SelectValue placeholder="选择专家" /></SelectTrigger><SelectContent>{availableExperts.map((expert) => <SelectItem key={expert.id} value={expert.id}>{expert.name} · v{expert.packageVersion}</SelectItem>)}</SelectContent></Select></label><label className="is-wide"><span>长期目标</span><Textarea value={objective} onChange={(event) => setObjective(event.target.value)} placeholder="描述持续职责、产出和验收边界" /></label></div></section>
            <section><header><span><ShieldCheck />作用域与配额</span><b>Host 强制执行</b></header><div className="fox-agent-editor-grid"><label><span>绑定项目</span><Select value={projectId} onValueChange={setProjectId}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="none">不绑定项目</SelectItem>{projects.map((project) => <SelectItem key={project.id} value={project.id}>{project.name}</SelectItem>)}</SelectContent></Select></label><label><span>每日运行次数</span><Input type="number" min="1" max="100" value={runsPerDay} onChange={(event) => setRunsPerDay(event.target.value)} /></label><label><span>每日 token</span><Input type="number" min="256" max="2000000" value={tokensPerDay} onChange={(event) => setTokensPerDay(event.target.value)} /></label><label><span>单次时长（分钟）</span><Input type="number" min="1" max="15" value={durationMinutes} onChange={(event) => setDurationMinutes(event.target.value)} /></label><label><span>单次工具调用</span><Input type="number" min="0" max="100" value={toolCalls} onChange={(event) => setToolCalls(event.target.value)} /></label></div></section>
            <section><header><span><ShieldCheck />远程知识绑定</span><b>{knowledgeIds.size} 已选择</b></header><div className="fox-agent-resource-list">{knowledgeBases.map((base) => <div key={base.id}><span><ShieldCheck /></span><p><b>{base.name}</b><small>{base.description || `${base.fileCount} 个文件`}</small></p><Switch checked={knowledgeIds.has(base.id)} onCheckedChange={(checked) => setKnowledgeIds((current) => { const next = new Set(current); if (checked) next.add(base.id); else next.delete(base.id); return next })} /></div>)}{!knowledgeBases.length && <p className="fox-agent-resource-empty">远程知识服务不可用；可以先创建，稍后重新部署带知识绑定的数字同事。</p>}</div></section>
            <DialogFooter><Button disabled={busy || !desktopRuntimeAvailable} onClick={() => void createColleague()}>{busy ? <LoaderCircle className="animate-spin" /> : <Plus />}部署数字同事</Button></DialogFooter>
          </TabsContent>
        </Tabs>
      </DialogContent>
    </Dialog>
  )
}
