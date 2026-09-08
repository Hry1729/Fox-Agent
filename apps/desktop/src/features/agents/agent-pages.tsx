import { useEffect, useMemo, useRef, useState } from 'react'
import { Brain, CalendarDays, CheckCircle2, Clock3, Code2, Database, Download, FileUp, Folder, Globe2, History, Images, Info, LoaderCircle, MessageSquareText, Plus, Puzzle, RotateCcw, Search, Settings2, Sparkles, Wrench } from 'lucide-react'
import { notify as toast } from '@/features/notifications'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { AgentRecord, ConversationSummary, ExpertPackagePreview, ExpertPackageVersionRecord, KnowledgeReference, MemoryEntityRecord, MemoryKind, MemoryRecallRecord, MemoryRevisionRecord, MemoryScope, ProjectRecord, SaveAgentInput } from '@/features/conversations/model/types'
import { AgentCard, type AgentCardData } from '@/features/workspace/entity-card'
import { WorkspacePage } from '@/features/workspace/page-shell'
import type { NavigateWorkspace } from '@/features/workspace/types'
import { useAgents } from './use-agents'
import { expertCatalogAgents } from './agent-classification'
import { useExpertIcons } from './use-expert-icons'
import { DigitalColleagueManager } from './digital-colleague-manager'
import { useMcpServers } from '@/features/settings/use-mcp-servers'
import { useSkills } from '@/features/settings/use-skills'
import { useKnowledgeBases } from '@/features/knowledge/use-knowledge'
import { knowledgeReferencesFromDeclaration, remoteKnowledgeReference, resolveExpertKnowledgeDeclaration, toPersistedExpertManifest } from './expert-knowledge'

function asCard(agent: AgentRecord): AgentCardData {
  const manifest = agent.packageManifest
  let knowledgeCount = agent.resources.knowledges.length
  if (agent.runtimeType === 'pi') {
    try {
      const declaration = resolveExpertKnowledgeDeclaration(manifest)
      knowledgeCount = declaration.format === 'references'
        ? declaration.references.length
        : declaration.format === 'legacy_remote_ids'
          ? declaration.ids.length
          : 0
    } catch {
      knowledgeCount = 0
    }
  }
  return {
    ...agent,
    image: agent.icon || (agent.runtimeType === 'pi' ? '/mascot/fox_magic.png' : '/mascot/fox_search.png'),
    tools: agent.runtimeType === 'pi' && Array.isArray(manifest.allowedTools) ? manifest.allowedTools.length : agent.resources.tools.length,
    knowledge: knowledgeCount,
    mcps: agent.runtimeType === 'pi' && Array.isArray(manifest.mcpServers) ? manifest.mcpServers.length : agent.resources.mcps.length,
    skills: agent.runtimeType === 'pi' && Array.isArray(manifest.skills) ? manifest.skills.length : agent.resources.skills.length,
    recent: agent.runtimeType === 'pi' ? '本机' : '知识库',
    active: agent.isDefault,
  }
}

const fallbackAgent: AgentRecord = { id: 'fox-general', name: 'Fox 通用助手', description: 'Fox 默认通用专家', runtimeType: 'pi', defaultModel: 'configured-model', icon: null, category: 'general', openingSuggestions: [], systemPrompt: '', isBuiltin: true, packageVersion: '1.0.0', packageSource: 'builtin', packageId: null, packageHash: null, packageManifest: {}, capabilities: ['files', 'tools', 'reasoning'], resources: { tools: [{ id: 'files', name: '文件读写', description: '读取和处理已授权项目文件' }, { id: 'tools', name: '本地工具', description: '调用 Fox Runtime 提供的本地工具' }, { id: 'reasoning', name: '任务推理', description: '规划并执行多步骤任务' }], knowledges: [], mcps: [], skills: [] }, configurableItems: {}, isDefault: true, available: true }

const emptyAgentDraft: SaveAgentInput = {
  name: '', description: '', icon: '/mascot/fox_magic.png', category: 'general', systemPrompt: '',
  defaultModel: 'configured-model', openingSuggestions: [],
  packageManifest: { version: '1.0.0', manifestSchemaVersion: 2, skills: [], knowledgeReferences: [], mcpServers: [], allowedTools: [] },
}

interface MemoryDraft {
  scope: MemoryScope
  scopeKey: string
  kind: MemoryKind
  canonicalKey: string
  content: string
  evidenceExcerpt: string
}

const emptyMemoryDraft: MemoryDraft = {
  scope: 'agent',
  scopeKey: '',
  kind: 'preference',
  canonicalKey: '',
  content: '',
  evidenceExcerpt: '',
}

const memoryKindLabels: Record<MemoryKind, string> = {
  preference: '偏好', identity: '身份', project: '项目', workflow: '工作流', fact: '事实', other: '其他',
}

const memoryStateLabels: Record<MemoryEntityRecord['state'], string> = {
  candidate: '待确认', confirmed: '已确认', conflict: '有冲突',
}

function memoryTime(value: number | null) {
  return value ? new Date(value).toLocaleString('zh-CN') : '从未'
}

function AgentMemoryManager({ agent }: { agent: AgentRecord }) {
  const [memories, setMemories] = useState<MemoryEntityRecord[]>([])
  const [projects, setProjects] = useState<ProjectRecord[]>([])
  const [loading, setLoading] = useState(true)
  const [query, setQuery] = useState('')
  const [editorOpen, setEditorOpen] = useState(false)
  const [editing, setEditing] = useState<MemoryEntityRecord | null>(null)
  const [draft, setDraft] = useState<MemoryDraft>(emptyMemoryDraft)
  const [saving, setSaving] = useState(false)
  const [historyOpen, setHistoryOpen] = useState(false)
  const [historyMemory, setHistoryMemory] = useState<MemoryEntityRecord | null>(null)
  const [revisions, setRevisions] = useState<MemoryRevisionRecord[]>([])
  const [recalls, setRecalls] = useState<MemoryRecallRecord[]>([])

  const refresh = async () => {
    if (!desktopRuntimeAvailable) return
    const [records, projectRecords] = await Promise.all([
      desktopClient.listMemories(),
      desktopClient.listProjects(),
    ])
    setMemories(records)
    setProjects(projectRecords)
  }

  useEffect(() => {
    let active = true
    if (!desktopRuntimeAvailable) {
      setLoading(false)
      return () => { active = false }
    }
    setLoading(true)
    void Promise.all([desktopClient.listMemories(), desktopClient.listProjects()])
      .then(([records, projectRecords]) => {
        if (!active) return
        setMemories(records)
        setProjects(projectRecords)
      })
      .catch((cause) => { if (active) toast.error(cause instanceof Error ? cause.message : String(cause)) })
      .finally(() => { if (active) setLoading(false) })
    return () => { active = false }
  }, [agent.id])

  const visibleMemories = useMemo(() => {
    const normalized = query.trim().toLowerCase()
    return memories.filter((memory) => {
      if (memory.scope === 'agent' && memory.scopeKey !== agent.id) return false
      return !normalized
        || memory.canonicalKey.toLowerCase().includes(normalized)
        || memory.content.toLowerCase().includes(normalized)
        || memory.evidenceExcerpt.toLowerCase().includes(normalized)
    })
  }, [agent.id, memories, query])

  const openCreate = () => {
    setEditing(null)
    setDraft({ ...emptyMemoryDraft, scopeKey: agent.id })
    setEditorOpen(true)
  }

  const openEdit = (memory: MemoryEntityRecord) => {
    setEditing(memory)
    setDraft({
      scope: memory.scope,
      scopeKey: memory.scopeKey,
      kind: memory.kind,
      canonicalKey: memory.canonicalKey,
      content: memory.content,
      evidenceExcerpt: memory.evidenceExcerpt,
    })
    setEditorOpen(true)
  }

  const saveMemory = async () => {
    if (!draft.canonicalKey.trim() || !draft.content.trim()) {
      toast.error('记忆键和值不能为空')
      return
    }
    if (draft.scope === 'project' && !draft.scopeKey) {
      toast.error('请选择项目作用域')
      return
    }
    setSaving(true)
    try {
      if (editing) {
        await desktopClient.updateMemory({
          memoryId: editing.id,
          kind: draft.kind,
          canonicalKey: draft.canonicalKey,
          content: draft.content,
          evidenceExcerpt: draft.evidenceExcerpt,
          expectedVersion: editing.version,
        })
      } else {
        await desktopClient.createMemory({
          scope: draft.scope,
          scopeKey: draft.scope === 'global' ? undefined : draft.scopeKey,
          kind: draft.kind,
          canonicalKey: draft.canonicalKey,
          content: draft.content,
          evidenceExcerpt: draft.evidenceExcerpt,
        })
      }
      await refresh()
      setEditorOpen(false)
      toast.success(editing ? '记忆已更新' : '记忆已创建并确认')
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setSaving(false)
    }
  }

  const mutate = async (operation: () => Promise<unknown>, success: string) => {
    try {
      await operation()
      await refresh()
      toast.success(success)
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause))
    }
  }

  const openHistory = async (memory: MemoryEntityRecord) => {
    setHistoryMemory(memory)
    setHistoryOpen(true)
    setRevisions([])
    setRecalls([])
    try {
      const [revisionRecords, recallRecords] = await Promise.all([
        desktopClient.listMemoryRevisions(memory.id),
        desktopClient.listMemoryRecalls(memory.id),
      ])
      setRevisions(revisionRecords)
      setRecalls(recallRecords)
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause))
    }
  }

  const scopeName = (memory: MemoryEntityRecord) => {
    if (memory.scope === 'global') return '全局'
    if (memory.scope === 'agent') return memory.scopeKey === agent.id ? `当前专家 · ${agent.name}` : `专家 · ${memory.scopeKey}`
    return `项目 · ${projects.find((project) => project.id === memory.scopeKey)?.name ?? memory.scopeKey}`
  }

  if (!desktopRuntimeAvailable) return <Card className="fox-agent-section-card fox-agent-memory-empty"><span><Brain /></span><h2>仅桌面版可用</h2><p>长期记忆由 Fox Host 本地治理，不会同步到网页 Runtime。</p></Card>

  return <div className="fox-memory-manager">
    <div className="fox-memory-toolbar">
      <div><Search size={15} /><Input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索记忆、证据或键" /></div>
      <Button size="sm" onClick={openCreate}><Plus size={14} />新增记忆</Button>
    </div>
    <Card className="fox-memory-privacy-note"><Brain size={18} /><p><b>用户控制的长期记忆</b><span>模型只能提交候选；只有已确认且启用的条目会被有界召回。Fox 不会自动保存整段对话。{agent.runtimeType !== 'pi' ? ' 当前远程 Runtime 仅支持查看和治理，尚不注入本地记忆。' : ''}</span></p></Card>
    {loading ? <Card className="fox-agent-section-card fox-agent-memory-empty"><LoaderCircle className="fox-spin" /><h2>正在读取记忆</h2></Card> : visibleMemories.length === 0 ? <Card className="fox-agent-section-card fox-agent-memory-empty"><span><Brain /></span><h2>暂无长期记忆</h2><p>你可以主动创建；Agent 提交的候选也会出现在这里等待确认。</p></Card> : <div className="fox-memory-list">{visibleMemories.map((memory) => <Card key={memory.id} className={`fox-memory-card is-${memory.state}`}>
      <header><div><Badge variant={memory.state === 'conflict' ? 'destructive' : 'secondary'}>{memoryStateLabels[memory.state]}</Badge><span>{memoryKindLabels[memory.kind]}</span><span>{scopeName(memory)}</span></div>{memory.state === 'confirmed' && <Switch checked={memory.enabled} aria-label={`${memory.canonicalKey} 记忆启用状态`} onCheckedChange={(enabled) => void mutate(() => desktopClient.setMemoryEnabled(memory.id, enabled, memory.version), enabled ? '记忆已启用' : '记忆已禁用')} />}</header>
      <h3>{memory.canonicalKey}</h3><p>{memory.content}</p>
      {memory.evidenceExcerpt && <blockquote>证据：{memory.evidenceExcerpt}</blockquote>}
      <small>来源：{memory.createdBy === 'agent' ? 'Agent 候选' : '用户'} · 置信度 {Math.round(memory.confidence * 100)}% · 召回 {memory.recallCount} 次 · 最近 {memoryTime(memory.lastRecalledAt)}</small>
      <footer>
        {memory.state === 'candidate' && !memory.openConflictId && <Button size="sm" onClick={() => void mutate(() => desktopClient.confirmMemory(memory.id, memory.version), '候选记忆已确认')}>确认并启用</Button>}
        {memory.state === 'conflict' && memory.openConflictId && <><Button size="sm" variant="outline" onClick={() => void mutate(() => desktopClient.resolveMemoryConflict(memory.openConflictId!, 'keep_existing'), '已保留原记忆')}>保留原记忆</Button><Button size="sm" onClick={() => void mutate(() => desktopClient.resolveMemoryConflict(memory.openConflictId!, 'accept_competing'), '已采用新候选')}>采用此候选</Button></>}
        <Button size="sm" variant="ghost" onClick={() => void openHistory(memory)}><History size={14} />审计记录</Button>
        <Button size="sm" variant="ghost" onClick={() => openEdit(memory)}>编辑</Button>
        <Button size="sm" variant="ghost" disabled={Boolean(memory.openConflictId)} onClick={() => void mutate(() => desktopClient.deleteMemory(memory.id), '记忆已删除')}>删除</Button>
      </footer>
    </Card>)}</div>}

    <Dialog open={editorOpen} onOpenChange={setEditorOpen}><DialogContent className="fox-memory-dialog"><DialogHeader><DialogTitle>{editing ? '编辑长期记忆' : '新增长期记忆'}</DialogTitle><DialogDescription>作用域创建后不可更改。用户新建条目会直接确认；同键不同值会进入冲突处理。</DialogDescription></DialogHeader>
      <div className="fox-memory-form">
        <label>作用域<Select disabled={Boolean(editing)} value={draft.scope} onValueChange={(scope: MemoryScope) => setDraft((current) => ({ ...current, scope, scopeKey: scope === 'agent' ? agent.id : scope === 'global' ? '' : projects[0]?.id ?? '' }))}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="global">全局</SelectItem><SelectItem value="agent">当前专家</SelectItem><SelectItem value="project">项目</SelectItem></SelectContent></Select></label>
        {draft.scope === 'project' && <label>项目<Select disabled={Boolean(editing)} value={draft.scopeKey} onValueChange={(scopeKey) => setDraft((current) => ({ ...current, scopeKey }))}><SelectTrigger><SelectValue placeholder="选择项目" /></SelectTrigger><SelectContent>{projects.map((project) => <SelectItem key={project.id} value={project.id}>{project.name}</SelectItem>)}</SelectContent></Select></label>}
        <label>类型<Select value={draft.kind} onValueChange={(kind: MemoryKind) => setDraft((current) => ({ ...current, kind }))}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent>{(Object.keys(memoryKindLabels) as MemoryKind[]).map((kind) => <SelectItem key={kind} value={kind}>{memoryKindLabels[kind]}</SelectItem>)}</SelectContent></Select></label>
        <label>规范键<Input value={draft.canonicalKey} maxLength={160} placeholder="例如 preferred_editor" onChange={(event) => setDraft((current) => ({ ...current, canonicalKey: event.target.value }))} /></label>
        <label>记忆内容<Textarea value={draft.content} maxLength={4000} placeholder="保存简洁、可复用的事实或偏好" onChange={(event) => setDraft((current) => ({ ...current, content: event.target.value }))} /></label>
        <label>证据摘录<Textarea value={draft.evidenceExcerpt} maxLength={1000} placeholder="为何保存这条记忆；不要粘贴整段对话" onChange={(event) => setDraft((current) => ({ ...current, evidenceExcerpt: event.target.value }))} /></label>
      </div><DialogFooter><Button variant="outline" onClick={() => setEditorOpen(false)}>取消</Button><Button disabled={saving} onClick={() => void saveMemory()}>{saving ? '正在保存' : '保存'}</Button></DialogFooter>
    </DialogContent></Dialog>

    <Dialog open={historyOpen} onOpenChange={setHistoryOpen}><DialogContent className="fox-memory-history-dialog"><DialogHeader><DialogTitle>记忆审计记录</DialogTitle><DialogDescription>{historyMemory?.canonicalKey} 的变更历史与召回证据。</DialogDescription></DialogHeader>
      <Tabs defaultValue="recalls"><TabsList><TabsTrigger value="recalls">召回记录 {recalls.length}</TabsTrigger><TabsTrigger value="revisions">变更记录 {revisions.length}</TabsTrigger></TabsList><TabsContent value="recalls"><div className="fox-memory-audit-list">{recalls.length === 0 ? <p>尚未被召回。</p> : recalls.map((recall) => <article key={recall.id}><b>{memoryTime(recall.recalledAt)} · 排名 {recall.rank}</b><span>查询：{recall.query || '空查询'}</span><span>原因：{recall.reason}</span>{recall.evidenceExcerpt && <span>证据：{recall.evidenceExcerpt}</span>}<small>Run：{recall.runId ?? '无'} · 对话：{recall.conversationId}</small></article>)}</div></TabsContent><TabsContent value="revisions"><div className="fox-memory-audit-list">{revisions.map((revision) => <article key={revision.id}><b>{memoryTime(revision.createdAt)} · {revision.action}</b><span>执行者：{revision.actor}</span></article>)}</div></TabsContent></Tabs>
    </DialogContent></Dialog>
  </div>
}

type ExpertResourceKey = 'skills' | 'knowledge' | 'mcpServers' | 'allowedTools'

const knowledgeToolIds = ['list_knowledge_bases', 'search_knowledge', 'read_knowledge_document', 'query_knowledge_graph']
const mcpToolIds = ['list_mcp_tools', 'call_mcp_tool']

interface ExpertResourceChoice {
  id: string
  name: string
  description: string
  badge: string
  icon: React.ReactNode
  disabled?: boolean
}

function ExpertResourceList({ items, selected, emptyText, disabled, onToggle }: {
  items: ExpertResourceChoice[]
  selected: Set<string>
  emptyText: string
  disabled: boolean
  onToggle: (id: string, checked: boolean) => void
}) {
  if (!items.length) return <p className="fox-agent-editor-resource-empty">{emptyText}</p>
  return <div className="fox-agent-editor-resource-list">{items.map((item) => (
    <div key={item.id} className="fox-agent-editor-resource-item">
      <span>{item.icon}</span>
      <p><b>{item.name}<em>{item.badge}</em></b><small>{item.description}</small></p>
      <Switch
        checked={selected.has(item.id)}
        disabled={disabled || item.disabled}
        aria-label={`${item.name} 专家资源`}
        onCheckedChange={(checked) => onToggle(item.id, checked)}
      />
    </div>
  ))}</div>
}

function localDayKey(date: Date) {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
}

function AgentWorkRecord({ agentId, conversationsOnly = false }: { agentId: string; conversationsOnly?: boolean }) {
  const [conversations, setConversations] = useState<ConversationSummary[]>([])

  useEffect(() => {
    let active = true
    void desktopClient.listConversations()
      .then((items) => { if (active) setConversations(items.filter((item) => item.agentId === agentId)) })
      .catch(() => { if (active) setConversations([]) })
    return () => { active = false }
  }, [agentId])

  const record = useMemo(() => {
    const today = new Date()
    today.setHours(0, 0, 0, 0)
    const mondayOffset = (today.getDay() + 6) % 7
    const start = new Date(today)
    start.setDate(today.getDate() - mondayOffset - 51 * 7)
    const activity = new Map<string, number>()
    conversations.forEach((conversation) => {
      const key = localDayKey(new Date(conversation.lastMessageAt ?? conversation.updatedAt))
      activity.set(key, (activity.get(key) ?? 0) + 1)
    })
    const days = Array.from({ length: 364 }, (_, index) => {
      const date = new Date(start)
      date.setDate(start.getDate() + index)
      const key = localDayKey(date)
      return { key, count: activity.get(key) ?? 0 }
    })
    const months: string[] = []
    for (let cursor = new Date(start.getFullYear(), start.getMonth(), 1); cursor <= today; cursor.setMonth(cursor.getMonth() + 1)) {
      months.push(`${cursor.getMonth() + 1}月`)
    }
    const projects = new Set(conversations.map((item) => item.projectId ?? item.projectRoot).filter(Boolean))
    return {
      activeDays: activity.size,
      projects: projects.size,
      days,
      months,
      maxActivity: Math.max(1, ...activity.values()),
      recent: [...conversations].sort((a, b) => (b.lastMessageAt ?? b.updatedAt) - (a.lastMessageAt ?? a.updatedAt)).slice(0, 6),
    }
  }, [conversations])

  const conversationList = record.recent.length ? record.recent.map((conversation) => <div key={conversation.id} className="fox-agent-task-row"><span className="fox-agent-task-icon"><MessageSquareText /></span><span className="fox-agent-task-copy"><b>{conversation.title}</b><small><Folder />{conversation.projectRoot?.split(/[\\/]/).filter(Boolean).at(-1) ?? '未关联项目'}</small></span><Badge variant="secondary"><CheckCircle2 />已完成</Badge><time>{new Date(conversation.lastMessageAt ?? conversation.updatedAt).toLocaleDateString('zh-CN')}</time></div>) : <p>还没有对话任务。</p>

  if (conversationsOnly) {
    return <Card className="fox-agent-work-card fox-agent-section-card"><div className="fox-agent-work-head"><h2>对话任务</h2></div><div className="fox-agent-work-list is-section-list">{conversationList}</div></Card>
  }

  return (
    <Card className="fox-agent-work-card">
      <Tabs defaultValue="timeline">
        <div className="fox-agent-work-head">
          <h2>工作记录</h2>
          <TabsList>
            <TabsTrigger value="timeline"><CalendarDays />时间线图</TabsTrigger>
            <TabsTrigger value="conversations"><History />对话任务</TabsTrigger>
          </TabsList>
        </div>
        <TabsContent value="timeline" className="fox-agent-work-content">
          <div className="fox-agent-work-metrics">
            <div><strong>{record.activeDays}<small>天</small></strong><span>活跃天数 <Info /></span></div>
            <div><strong>{conversations.length}</strong><span>对话任务 <Info /></span></div>
            <div><strong>{record.projects}</strong><span>参与项目 <Info /></span></div>
          </div>
          <div className="fox-agent-activity-panel">
            <div className="fox-agent-activity-months">{record.months.map((month, index) => <span key={`${month}-${index}`}>{month}</span>)}</div>
            <div className="fox-agent-activity-chart">
              <div className="fox-agent-activity-weekdays"><span>周一</span><span>周三</span><span>周五</span><span>周日</span></div>
              <div className="fox-agent-activity-grid">{record.days.map((day) => {
                const level = day.count === 0 ? 0 : Math.max(1, Math.ceil(day.count / record.maxActivity * 4))
                return <i key={day.key} className={`is-level-${level}`} title={`${day.key} · ${day.count} 个对话`} />
              })}</div>
            </div>
            <div className="fox-agent-activity-legend"><span>少</span>{[0, 1, 2, 3, 4].map((level) => <i key={level} className={`is-level-${level}`} />)}<span>多</span></div>
          </div>
        </TabsContent>
        <TabsContent value="conversations" className="fox-agent-work-list">
          {conversationList}
        </TabsContent>
      </Tabs>
    </Card>
  )
}

const foxRuntimeTools = [
  { id: 'read', name: 'read', description: '读取已授权项目中的 UTF-8 文本文件', kind: 'Runtime' },
  { id: 'ls', name: 'ls', description: '列出指定目录下的文件和文件夹', kind: 'Runtime' },
  { id: 'find', name: 'find', description: '按名称在项目目录中查找文件和文件夹', kind: 'Runtime' },
  { id: 'grep', name: 'grep', description: '在项目文本文件中搜索指定内容', kind: 'Runtime' },
  { id: 'read_attachment', name: 'read_attachment', description: '读取当前对话中的文本或 DOCX 附件', kind: 'Runtime' },
  { id: 'write_file', name: 'write_file', description: '在已授权项目中创建或覆盖文本文件', kind: 'Runtime' },
  { id: 'edit_file', name: 'edit_file', description: '替换文件中的指定文本并生成变更记录', kind: 'Runtime' },
  { id: 'run_command', name: 'run_command', description: '在已授权项目目录中执行非交互式 CLI 命令', kind: 'CLI' },
  { id: 'web_search', name: 'web_search', description: '检索实时网页，默认无需额外搜索密钥', kind: 'Web' },
  { id: 'web_read', name: 'web_read', description: '安全读取公开网页并提取正文', kind: 'Web' },
  { id: 'structured_data', name: 'structured_data', description: '解析、转换、查询和校验结构化数据', kind: 'Data' },
  { id: 'git_read', name: 'git_read', description: '只读查看 Git 状态、差异、历史和逐行归属', kind: 'CLI' },
  { id: 'test_run', name: 'test_run', description: '识别项目类型并运行已有测试命令', kind: 'CLI' },
  { id: 'code_check', name: 'code_check', description: '运行项目已有的 lint、类型检查和静态检查', kind: 'CLI' },
  { id: 'format_code', name: 'format_code', description: '使用项目已有格式化器检查或应用格式', kind: 'CLI' },
  { id: 'tabular_data', name: 'tabular_data', description: '预览、筛选和聚合 CSV、TSV 与 JSON 表格', kind: 'Data' },
  { id: 'list_knowledge_bases', name: 'list_knowledge_bases', description: '列出当前对话已绑定的知识库', kind: 'Knowledge' },
  { id: 'search_knowledge', name: 'search_knowledge', description: '检索已绑定知识库并返回来源', kind: 'Knowledge' },
  { id: 'read_knowledge_document', name: 'read_knowledge_document', description: '读取知识库中已解析的文档', kind: 'Knowledge' },
  { id: 'query_knowledge_graph', name: 'query_knowledge_graph', description: '查询知识图谱中的有限子图', kind: 'Knowledge' },
  { id: 'list_mcp_tools', name: 'list_mcp_tools', description: '读取当前已启用 连接器提供的工具清单', kind: 'MCP' },
  { id: 'call_mcp_tool', name: 'call_mcp_tool', description: '通过 Fox 权限审批调用指定的 连接器工具', kind: 'MCP' },
  { id: 'work_snapshot_get', name: 'work_snapshot_get', description: '读取当前目标、任务、证据和 A1 验收状态', kind: 'Work' },
  { id: 'goal_propose', name: 'goal_propose', description: '向 Host 提议一个可跟踪目标', kind: 'Work' },
  { id: 'goal_complete', name: 'goal_complete', description: '兼容 A0 的目标完成请求', kind: 'Work' },
  { id: 'task_create_many', name: 'task_create_many', description: '为已激活目标创建有序任务', kind: 'Work' },
  { id: 'task_update', name: 'task_update', description: '更新任务状态与乐观版本', kind: 'Work' },
  { id: 'task_evidence_add', name: 'task_evidence_add', description: '为任务登记可验证证据', kind: 'Work' },
  { id: 'task_evidence_validate', name: 'task_evidence_validate', description: '重新验证任务证据', kind: 'Work' },
  { id: 'plan_revision_create', name: 'plan_revision_create', description: '持久化完整计划修订', kind: 'Work' },
  { id: 'review_finding_add', name: 'review_finding_add', description: '登记独立审查发现', kind: 'Work' },
  { id: 'review_finding_resolve', name: 'review_finding_resolve', description: '解决或豁免审查发现', kind: 'Work' },
  { id: 'acceptance_submit', name: 'acceptance_submit', description: '提交 A1 最终验收', kind: 'Work' },
  { id: 'workflow_snapshot_get', name: 'workflow_snapshot_get', description: '读取持久化专家工作流检查点', kind: 'Workflow' },
  { id: 'workflow_start', name: 'workflow_start', description: '按能力包声明启动版本化工作流', kind: 'Workflow' },
  { id: 'workflow_stage_start', name: 'workflow_stage_start', description: '启动或重试当前工作流阶段', kind: 'Workflow' },
  { id: 'workflow_stage_complete', name: 'workflow_stage_complete', description: '校验并持久化阶段输出', kind: 'Workflow' },
  { id: 'workflow_stage_fail', name: 'workflow_stage_fail', description: '登记阶段失败并应用有限重试', kind: 'Workflow' },
  { id: 'workflow_cancel', name: 'workflow_cancel', description: '取消活动工作流并收口任务状态', kind: 'Workflow' },
]

const toolDescriptions: Record<string, string> = {
  read: '读取文件',
  read_file: '读取文件',
  ls: '列出目录中的文件和文件夹',
  list_files: '列出目录中的文件和文件夹',
  find: '按名称查找文件和文件夹',
  find_files: '按名称查找文件和文件夹',
  grep: '在文本文件中搜索内容',
  search_files: '在文本文件中搜索内容',
  read_attachment: '读取对话附件内容',
  write_file: '创建或覆盖文本文件',
  edit_file: '编辑文件中的指定内容',
  run_command: '执行 CLI 命令',
  web_search: '检索实时网页',
  web_read: '读取公开网页正文',
  structured_data: '处理结构化数据',
  git_read: '读取 Git 状态与历史',
  test_run: '运行项目测试',
  code_check: '运行代码质量检查',
  format_code: '检查或应用代码格式',
  tabular_data: '预览、筛选和聚合表格数据',
  list_mcp_tools: '列出 连接器提供的工具',
  call_mcp_tool: '调用 连接器中的工具',
}

function AgentCapabilitiesCard({ agent, navigate, section = 'all' }: { agent: AgentCardData; navigate: NavigateWorkspace; section?: 'all' | 'skills' | 'tools' }) {
  const skillResource = useSkills(agent.id)
  const mcpResource = useMcpServers()
  const [managerOpen, setManagerOpen] = useState(false)
  const [managerTab, setManagerTab] = useState<'skills' | 'tools'>('skills')
  const [draftSkills, setDraftSkills] = useState<Set<string>>(new Set())
  const [draftMcps, setDraftMcps] = useState<Set<string>>(new Set())
  const [draftTools, setDraftTools] = useState<Set<string>>(new Set())
  const [saving, setSaving] = useState(false)
  const localAgent = agent.runtimeType === 'pi'
  const editableLocal = localAgent && agent.packageSource === 'local'
  const defaultSkillIds = new Set(agent.resources.skills.map((item) => item.id))
  const defaultSkills = agent.resources.skills
  const userSkills = localAgent ? skillResource.items.filter((item) => !defaultSkillIds.has(item.id)) : []
  const skills = localAgent
    ? [...defaultSkills, ...userSkills.filter((item) => item.enabled).map((item) => ({ id: item.id, name: item.name, description: item.description }))]
    : agent.resources.skills
  const manifestMcpIds = Array.isArray(agent.packageManifest.mcpServers) ? new Set(agent.packageManifest.mcpServers) : null
  const enabledMcps = mcpResource.items.filter((item) => item.enabled && (agent.isBuiltin || !manifestMcpIds || manifestMcpIds.has(item.id)))
  const allowedTools = Array.isArray(agent.packageManifest.allowedTools) ? new Set(agent.packageManifest.allowedTools) : null
  const tools = agent.runtimeType === 'pi'
    ? [...foxRuntimeTools.filter((tool) => !allowedTools || allowedTools.has(tool.id)), ...enabledMcps.map((item) => ({ id: item.id, name: item.name, description: item.command, kind: 'MCP' }))]
    : [
        ...agent.resources.tools.map((item) => ({ ...item, kind: 'Tool' })),
        ...agent.resources.mcps.map((item) => ({ ...item, kind: 'MCP' })),
      ]

  const openManager = (tab: 'skills' | 'tools') => {
    setManagerTab(tab)
    setDraftSkills(new Set(userSkills.filter((item) => item.enabled).map((item) => item.id)))
    setDraftMcps(new Set(agent.isBuiltin
      ? mcpResource.items.filter((item) => item.enabled).map((item) => item.id)
      : Array.isArray(agent.packageManifest.mcpServers) ? agent.packageManifest.mcpServers : []))
    setDraftTools(new Set(Array.isArray(agent.packageManifest.allowedTools) ? agent.packageManifest.allowedTools : foxRuntimeTools.map((tool) => tool.id)))
    setManagerOpen(true)
  }

  const manageResources = (tab: 'skills' | 'tools') => {
    if (section === 'all') {
      navigate('agent-detail', agent.id, { documentId: tab })
      return
    }
    openManager(tab)
  }

  const toggleDraft = (kind: 'skills' | 'tools' | 'mcp', id: string, enabled: boolean) => {
    const update = (current: Set<string>) => {
      const next = new Set(current)
      if (enabled) next.add(id)
      else next.delete(id)
      return next
    }
    if (kind === 'skills') setDraftSkills(update)
    else if (kind === 'tools') setDraftTools(update)
    else setDraftMcps(update)
  }

  const saveResources = async () => {
    if (!editableLocal) return
    setSaving(true)
    const skillChanges = userSkills.filter((item) => draftSkills.has(item.id) !== item.enabled)
    const mcpChanges = agent.isBuiltin
      ? mcpResource.items.filter((item) => draftMcps.has(item.id) !== item.enabled)
      : mcpResource.items.filter((item) => draftMcps.has(item.id) && !item.enabled)
    const results = await Promise.all([
      ...skillChanges.map((item) => skillResource.setEnabled(item.id, draftSkills.has(item.id))),
      ...mcpChanges.map((item) => mcpResource.setEnabled(item.id, draftMcps.has(item.id))),
    ])
    if (!agent.isBuiltin) {
      try {
        await desktopClient.saveAgent({
          id: agent.id, name: agent.name, description: agent.description, icon: agent.icon,
          category: agent.category, systemPrompt: agent.systemPrompt, defaultModel: agent.defaultModel,
          openingSuggestions: agent.openingSuggestions,
          packageManifest: toPersistedExpertManifest({ ...agent.packageManifest, skills: [...draftSkills], mcpServers: [...draftMcps], allowedTools: [...draftTools] }),
        })
      } catch {
        results.push(false)
      }
    }
    setSaving(false)
    if (results.some((result) => !result)) {
      toast.error('部分能力或工具未能保存，请检查扩展配置')
      return
    }
    setManagerOpen(false)
    toast.success('能力与工具配置已更新')
  }

  return (
    <>
      <Card className="fox-agent-capabilities-card">
        <h2>{section === 'skills' ? '技能' : section === 'tools' ? '工具' : '能力与工具'}</h2>
        {section !== 'tools' && <section>
          <header><h3>能力（{skills.length}）</h3><Button variant="outline" size="sm" onClick={() => manageResources('skills')}><Settings2 />管理</Button></header>
          <div className="fox-agent-capability-grid">{skills.length ? skills.map((skill) => <div key={skill.id}><span><Puzzle /></span><p><b>{skill.name || skill.id}</b><small>{skill.description || `技能 · ${skill.id}`}</small></p><Badge variant="outline">技能</Badge></div>) : <p className="fox-agent-capability-empty">此专家暂未配置 技能。</p>}</div>
        </section>}
        {section !== 'skills' && <section>
          <header><h3>工具（{tools.length}）</h3><Button variant="outline" size="sm" onClick={() => manageResources('tools')}><Settings2 />管理</Button></header>
          <div className="fox-agent-capability-grid">{tools.length ? tools.slice(0, 16).map((tool) => {
            const toolName = tool.id || tool.name
            const description = tool.description || toolDescriptions[toolName] || `调用 ${toolName} 工具`
            return <div key={`${tool.kind}:${toolName}`}><span>{tool.kind === 'CLI' ? <Code2 /> : tool.kind === 'MCP' ? <Globe2 /> : /file|read|write|edit/i.test(toolName) ? <Wrench /> : <Search />}</span><p><b><code>{toolName}</code></b><small>{description}</small></p><Badge variant="outline">{tool.kind}</Badge></div>
          }) : <p className="fox-agent-capability-empty">当前 Runtime 尚未返回 Tool、CLI 或 连接器工具。</p>}</div>
        </section>}
      </Card>
      <Dialog open={managerOpen} onOpenChange={setManagerOpen}>
        <DialogContent className="fox-agent-resource-dialog">
          <DialogHeader>
            <div className="fox-agent-resource-dialog-title"><div><DialogTitle>管理能力与工具</DialogTitle><DialogDescription>{editableLocal ? '预置资源默认开启且不可修改，用户添加的资源可以自由启用或停用。' : agent.packageSource === 'imported' ? '已安装能力包的资源声明不可直接修改；可复制为本地专家后编辑。' : '该专家的能力与工具由其来源统一管理。'}</DialogDescription></div>{editableLocal && <Button variant="outline" size="sm" onClick={() => { setManagerOpen(false); navigate(managerTab === 'skills' ? 'skills' : 'mcp') }}><Plus />{managerTab === 'skills' ? '添加 技能' : '添加工具'}</Button>}</div>
          </DialogHeader>
          <Tabs value={managerTab} onValueChange={(value) => setManagerTab(value as 'skills' | 'tools')}>
            <TabsList><TabsTrigger value="skills">能力（{localAgent ? defaultSkills.length + draftSkills.size : skills.length}）</TabsTrigger><TabsTrigger value="tools">工具（{localAgent ? foxRuntimeTools.length + draftMcps.size : tools.length}）</TabsTrigger></TabsList>
            <TabsContent value="skills" className="fox-agent-resource-list">
              {localAgent ? <>{defaultSkills.map((skill) => <div key={`default:${skill.id}`}><span><Puzzle /></span><p><b>{skill.name || skill.id}<em>预置</em></b><small>{skill.description || `技能 · ${skill.id}`}</small></p><Switch checked disabled aria-label={`${skill.name} 默认开启`} /></div>)}{userSkills.map((skill) => <div key={skill.id}><span><Puzzle /></span><p><b>{skill.name}<em>用户添加</em></b><small>{skill.description || skill.id}</small></p><Switch checked={draftSkills.has(skill.id)} disabled={!editableLocal || !skill.valid || saving} onCheckedChange={(checked) => toggleDraft('skills', skill.id, checked)} /></div>)}{!defaultSkills.length && !userSkills.length && <p className="fox-agent-resource-empty">还没有可用的 技能。</p>}</> : skills.length ? skills.map((skill) => <div key={skill.id}><span><Puzzle /></span><p><b>{skill.name}<em>预置</em></b><small>{skill.description}</small></p><Switch checked disabled aria-label={`${skill.name} 默认开启`} /></div>) : <p className="fox-agent-resource-empty">该专家没有公开的能力信息。</p>}
            </TabsContent>
            <TabsContent value="tools" className="fox-agent-resource-list">
              {localAgent ? <>{foxRuntimeTools.map((tool) => <div key={tool.id}><span>{tool.kind === 'CLI' ? <Code2 /> : <Wrench />}</span><p><b><code>{tool.id}</code><em>{agent.isBuiltin ? '预置' : '能力包'}</em></b><small>{tool.description}</small></p><Switch checked={draftTools.has(tool.id)} disabled={!editableLocal || saving} aria-label={`${tool.id} 工具权限`} onCheckedChange={(checked) => toggleDraft('tools', tool.id, checked)} /></div>)}{mcpResource.items.map((server) => <div key={server.id}><span><Globe2 /></span><p><b>{server.name}<em>用户添加</em></b><small>MCP · {server.command}</small></p><Switch checked={draftMcps.has(server.id)} disabled={!editableLocal || saving} onCheckedChange={(checked) => toggleDraft('mcp', server.id, checked)} /></div>)}</> : tools.length ? tools.map((tool) => <div key={`${tool.kind}:${tool.id}`}><span><Wrench /></span><p><b><code>{tool.id}</code><em>预置</em></b><small>{tool.description}</small></p><Switch checked disabled aria-label={`${tool.id} 默认开启`} /></div>) : <p className="fox-agent-resource-empty">该专家没有公开的工具信息。</p>}
            </TabsContent>
          </Tabs>
          <DialogFooter><Button variant="outline" onClick={() => setManagerOpen(false)}>取消</Button><Button disabled={!editableLocal || saving} onClick={() => void saveResources()}>{saving ? '正在保存' : '保存更改'}</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}

function AgentSectionLayout({ title, description, children }: { title: string; description: string; children: React.ReactNode }) {
  return <section className="fox-agent-section-layout"><header><h1>{title}</h1><p>{description}</p></header>{children}</section>
}

export function AgentListPage({ sidebarCollapsed, onSidebar, navigate }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace }) {
  const resource = useAgents()
  const cards = expertCatalogAgents(resource.agents).map(asCard)
  const [query, setQuery] = useState('')
  const [availability, setAvailability] = useState<'all' | 'available' | 'unavailable'>('all')
  const [environment, setEnvironment] = useState<'all' | 'local' | 'knowledge'>('all')
  const [category, setCategory] = useState('all')
  const [editorOpen, setEditorOpen] = useState(false)
  const [editingAgent, setEditingAgent] = useState<AgentRecord | null>(null)
  const [draft, setDraft] = useState<SaveAgentInput>(emptyAgentDraft)
  const [savingAgent, setSavingAgent] = useState(false)
  const [editorTab, setEditorTab] = useState<'profile' | 'resources' | 'versions'>('profile')
  const packageInputRef = useRef<HTMLInputElement>(null)
  const [packagePreviewOpen, setPackagePreviewOpen] = useState(false)
  const [packagePreview, setPackagePreview] = useState<ExpertPackagePreview | null>(null)
  const [pendingPackage, setPendingPackage] = useState<Record<string, unknown> | null>(null)
  const [packageVersions, setPackageVersions] = useState<ExpertPackageVersionRecord[]>([])
  const [packageBusy, setPackageBusy] = useState(false)
  const [digitalColleaguesOpen, setDigitalColleaguesOpen] = useState(false)
  const skillResource = useSkills(editingAgent?.id ?? 'fox-general')
  const mcpResource = useMcpServers()
  const knowledgeResource = useKnowledgeBases(editorOpen)
  const iconResource = useExpertIcons(editorOpen)
  const normalizedQuery = query.trim().toLocaleLowerCase()
  const filteredCards = cards.filter((agent) => {
    if (availability === 'available' && !agent.available) return false
    if (availability === 'unavailable' && agent.available) return false
    if (environment === 'local' && agent.runtimeType !== 'pi') return false
    if (environment === 'knowledge' && agent.runtimeType === 'pi') return false
    if (category !== 'all' && agent.category !== category) return false
    return !normalizedQuery || [agent.name, agent.description, agent.defaultModel].some((value) => value.toLocaleLowerCase().includes(normalizedQuery))
  })
  const openCreate = () => {
    setEditingAgent(null)
    setDraft({ ...emptyAgentDraft, packageManifest: { ...emptyAgentDraft.packageManifest } })
    setEditorTab('profile')
    setEditorOpen(true)
  }
  const openManage = (agent: AgentRecord) => {
    setEditingAgent(agent)
    setDraft({
      id: agent.id,
      name: agent.name,
      description: agent.description,
      icon: agent.icon,
      category: agent.category,
      systemPrompt: agent.systemPrompt,
      defaultModel: agent.defaultModel,
      openingSuggestions: agent.openingSuggestions,
      packageManifest: agent.packageManifest,
    })
    setEditorTab('profile')
    setEditorOpen(true)
    setPackageVersions([])
    if (agent.packageSource === 'imported') {
      void desktopClient.listExpertPackageVersions(agent.id)
        .then(setPackageVersions)
        .catch((cause) => toast.error('读取能力包版本失败', { description: cause instanceof Error ? cause.message : String(cause) }))
    }
  }

  const inspectPackageFile = async (file: File | undefined) => {
    if (!file) return
    if (file.size > 2 * 1024 * 1024) {
      toast.error('能力包不能超过 2 MiB')
      return
    }
    setPackageBusy(true)
    try {
      const parsed = JSON.parse(await file.text()) as unknown
      if (!parsed || Array.isArray(parsed) || typeof parsed !== 'object') throw new Error('能力包根节点必须是 JSON 对象')
      const expertPackage = parsed as Record<string, unknown>
      const preview = await desktopClient.inspectExpertPackage(expertPackage)
      setPendingPackage(expertPackage)
      setPackagePreview(preview)
      setPackagePreviewOpen(true)
    } catch (cause) {
      toast.error('能力包校验失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally {
      setPackageBusy(false)
      if (packageInputRef.current) packageInputRef.current.value = ''
    }
  }

  const installPackage = async () => {
    if (!pendingPackage || !packagePreview?.canInstall) return
    setPackageBusy(true)
    try {
      const installed = await desktopClient.installExpertPackage(pendingPackage, packagePreview.currentHash)
      await resource.refresh(false)
      setPackagePreviewOpen(false)
      setPendingPackage(null)
      setPackagePreview(null)
      toast.success(packagePreview.action === 'upgrade' ? `已升级到 v${installed.packageVersion}` : `已安装 ${installed.name}`)
    } catch (cause) {
      toast.error('安装能力包失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally {
      setPackageBusy(false)
    }
  }

  const exportPackage = async () => {
    if (!editingAgent) return
    setPackageBusy(true)
    try {
      const expertPackage = await desktopClient.exportExpertPackage(editingAgent.id)
      const packageId = typeof expertPackage.id === 'string' ? expertPackage.id : editingAgent.id
      const version = typeof expertPackage.version === 'string' ? expertPackage.version : editingAgent.packageVersion
      const url = URL.createObjectURL(new Blob([JSON.stringify(expertPackage, null, 2)], { type: 'application/json' }))
      const anchor = document.createElement('a')
      anchor.href = url
      anchor.download = `${packageId}-${version}.foxexpert`
      anchor.click()
      URL.revokeObjectURL(url)
      toast.success('能力包已导出')
    } catch (cause) {
      toast.error('导出能力包失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally {
      setPackageBusy(false)
    }
  }

  const rollbackPackage = async (version: string) => {
    if (!editingAgent || editingAgent.packageSource !== 'imported') return
    if (!window.confirm(`确定将“${editingAgent.name}”回滚到 v${version} 吗？现有对话仍保留原版本快照。`)) return
    setPackageBusy(true)
    try {
      const rolledBack = await desktopClient.rollbackExpertPackage(editingAgent.id, version, editingAgent.packageHash)
      await resource.refresh(false)
      const versions = await desktopClient.listExpertPackageVersions(editingAgent.id)
      setEditingAgent(rolledBack)
      setDraft((value) => ({ ...value, name: rolledBack.name, description: rolledBack.description, icon: rolledBack.icon, category: rolledBack.category, systemPrompt: rolledBack.systemPrompt, defaultModel: rolledBack.defaultModel, openingSuggestions: rolledBack.openingSuggestions, packageManifest: rolledBack.packageManifest }))
      setPackageVersions(versions)
      toast.success(`已回滚到 v${version}`)
    } catch (cause) {
      toast.error('回滚能力包失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally {
      setPackageBusy(false)
    }
  }
  const selectedResources = (key: ExpertResourceKey) => new Set(
    key === 'knowledge'
      ? (() => {
          try {
            return knowledgeReferencesFromDeclaration(resolveExpertKnowledgeDeclaration(draft.packageManifest)).map((reference) => reference.id)
          } catch {
            return []
          }
        })()
      : Array.isArray(draft.packageManifest[key]) ? draft.packageManifest[key] as string[] : [],
  )
  const toggleResource = (key: ExpertResourceKey, id: string, checked: boolean) => {
    setDraft((value) => {
      let selected: Set<string>
      let packageManifest = { ...value.packageManifest }
      if (key === 'knowledge') {
        let references = [] as ReturnType<typeof knowledgeReferencesFromDeclaration>
        try {
          references = knowledgeReferencesFromDeclaration(resolveExpertKnowledgeDeclaration(value.packageManifest))
        } catch {
          references = []
        }
        const remoteReferences = references.filter(
          (reference): reference is Extract<KnowledgeReference, { source: 'remote' }> => reference.source === 'remote',
        )
        selected = new Set(references.map((reference) => reference.id))
        if (checked) selected.add(id)
        else selected.delete(id)
        const nextRemoteReferences = remoteReferences.filter((reference) => selected.has(reference.id))
        if (checked && !remoteReferences.some((reference) => reference.id === id)) nextRemoteReferences.push(remoteKnowledgeReference(id))
        packageManifest.manifestSchemaVersion = 2
        packageManifest.knowledgeReferences = [
          ...references.filter((reference) => reference.source === 'local'),
          ...nextRemoteReferences,
        ]
        delete packageManifest.knowledge
      } else {
        selected = new Set(Array.isArray(value.packageManifest[key]) ? value.packageManifest[key] as string[] : [])
        if (checked) selected.add(id)
        else selected.delete(id)
        packageManifest = { ...packageManifest, [key]: [...selected] }
      }
      if (key === 'knowledge' || key === 'mcpServers') {
        const tools = new Set(Array.isArray(packageManifest.allowedTools) ? packageManifest.allowedTools : [])
        const dependentTools = key === 'knowledge' ? knowledgeToolIds : mcpToolIds
        if (selected.size) dependentTools.forEach((tool) => tools.add(tool))
        else dependentTools.forEach((tool) => tools.delete(tool))
        packageManifest.allowedTools = [...tools]
      }
      return { ...value, packageManifest }
    })
  }
  const saveAgent = async () => {
    if (!draft.name.trim() || !draft.systemPrompt.trim()) {
      toast.error('请填写专家名称和系统提示词')
      return
    }
    setSavingAgent(true)
    try {
      await desktopClient.saveAgent({ ...draft, packageManifest: toPersistedExpertManifest(draft.packageManifest) })
      await resource.refresh(false)
      setEditorOpen(false)
      toast.success(editingAgent ? '专家已更新' : '专家已创建')
    } catch (cause) {
      toast.error('保存专家失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally {
      setSavingAgent(false)
    }
  }
  const copyAgent = async () => {
    if (!editingAgent) return
    setSavingAgent(true)
    try {
      const copied = await desktopClient.copyAgent(editingAgent.id)
      await resource.refresh(false)
      openManage(copied)
      toast.success('已创建专家副本，可继续编辑')
    } catch (cause) {
      toast.error('复制专家失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally {
      setSavingAgent(false)
    }
  }
  const deleteAgent = async () => {
    if (!editingAgent || editingAgent.isBuiltin) return
    if (!window.confirm(`确定删除专家“${editingAgent.name}”吗？`)) return
    setSavingAgent(true)
    try {
      await desktopClient.deleteAgent(editingAgent.id)
      await resource.refresh(false)
      setEditorOpen(false)
      toast.success('专家已删除')
    } catch (cause) {
      toast.error('删除专家失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally {
      setSavingAgent(false)
    }
  }
  const readOnly = Boolean(editingAgent && (editingAgent.packageSource !== 'local' || editingAgent.runtimeType !== 'pi'))
  const iconOptions = draft.icon && !iconResource.items.some((item) => item.src === draft.icon)
    ? [{ id: 'current', name: '当前头像', src: draft.icon }, ...iconResource.items]
    : iconResource.items
  const knowledgeChoices: ExpertResourceChoice[] = knowledgeResource.items.map((item) => ({
    id: item.id,
    name: item.name,
    description: item.description || `${item.fileCount} 个文件`,
    badge: item.status || item.kbType || '知识库',
    icon: <Database />,
  }))
  const skillChoices: ExpertResourceChoice[] = skillResource.items.map((item) => ({
    id: item.id,
    name: item.name || item.id,
    description: item.validationError || item.description || `技能 · ${item.id}`,
    badge: item.valid ? `v${item.version}` : '不可用',
    icon: <Puzzle />,
    disabled: !item.valid,
  }))
  const mcpChoices: ExpertResourceChoice[] = mcpResource.items.map((item) => ({
    id: item.id,
    name: item.name,
    description: item.lastError || `${item.command} ${item.args.join(' ')}`.trim(),
    badge: item.enabled ? item.status : '未启用',
    icon: <Globe2 />,
    disabled: !item.enabled,
  }))
  const toolChoices: ExpertResourceChoice[] = foxRuntimeTools.map((item) => ({
    id: item.id,
    name: item.name,
    description: item.description,
    badge: item.kind,
    icon: item.kind === 'CLI' ? <Code2 /> : <Wrench />,
  }))
  return (
    <WorkspacePage className="fox-agent-list-page" title="专家" subtitle="查看并召唤本地与知识库服务中的专家" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
      <section className="fox-page-content fox-agent-list-page">
        <div className="fox-page-intro fox-list-page-intro"><h1>选择一个专家</h1><label className="fox-list-page-search"><Search size={14} /><Input value={query} onChange={(event) => setQuery(event.target.value)} aria-label="搜索专家" placeholder="搜索专家..." /></label></div>
        <div className="fox-page-toolbar fox-agent-toolbar">
          <Select value={availability} onValueChange={(value) => setAvailability(value as typeof availability)}><SelectTrigger className="fox-agent-filter"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">全部状态</SelectItem><SelectItem value="available">可使用</SelectItem><SelectItem value="unavailable">不可用</SelectItem></SelectContent></Select>
          <Select value={environment} onValueChange={(value) => setEnvironment(value as typeof environment)}><SelectTrigger className="fox-agent-filter"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">全部环境</SelectItem><SelectItem value="local">本机</SelectItem><SelectItem value="knowledge">知识库</SelectItem></SelectContent></Select>
          <Select value={category} onValueChange={setCategory}><SelectTrigger className="fox-agent-filter"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">全部分类</SelectItem><SelectItem value="general">通用</SelectItem><SelectItem value="engineering">工程</SelectItem><SelectItem value="design">设计</SelectItem><SelectItem value="planning">规划</SelectItem><SelectItem value="review">审查</SelectItem><SelectItem value="business">业务</SelectItem></SelectContent></Select>
          <span className="fox-agent-result-count">{filteredCards.length} 个结果</span>
          <Button variant="ghost" size="sm" disabled={resource.syncing} onClick={() => void resource.refresh()}>{resource.syncing && <LoaderCircle className="animate-spin" />}刷新</Button>
          <input ref={packageInputRef} hidden type="file" accept=".foxexpert,.json,application/json" onChange={(event) => void inspectPackageFile(event.target.files?.[0])} />
          <Button variant="outline" size="sm" disabled={packageBusy || !desktopRuntimeAvailable} onClick={() => packageInputRef.current?.click()}>{packageBusy ? <LoaderCircle className="animate-spin" /> : <FileUp size={14} />}导入能力包</Button>
          <Button variant="outline" size="sm" disabled={!desktopRuntimeAvailable} onClick={() => setDigitalColleaguesOpen(true)}><Clock3 size={14} />数字同事</Button>
          <Button size="sm" onClick={openCreate}><Plus size={14} />新建专家</Button>
        </div>
        {resource.error && <p className="fox-page-error">{resource.error}</p>}
        {(resource.syncing || resource.syncError) && <div className={`fox-agent-sync-notice ${resource.syncError ? 'is-offline' : ''}`} title={resource.syncError ?? undefined}>{resource.syncing ? <LoaderCircle className="animate-spin" /> : <Info />}<span>{resource.syncing ? '正在后台同步知识库专家，本地专家可正常使用。' : '知识库服务暂不可用或未登录，已显示缓存专家。'}</span></div>}
        {resource.loading && cards.length === 0 ? <p className="fox-page-empty fox-agent-list-empty">正在加载专家...</p> : filteredCards.length ? <div className="fox-entity-grid fox-shadcn-entity-grid">{filteredCards.map((agent) => <AgentCard key={agent.id} agent={agent} onUse={() => navigate('chat', agent.id)} onManage={agent.runtimeType === 'pi' ? () => openManage(agent) : undefined} />)}</div> : <p className="fox-page-empty fox-agent-list-empty">没有符合当前筛选条件的专家。</p>}
      </section>
      <DigitalColleagueManager open={digitalColleaguesOpen} onOpenChange={setDigitalColleaguesOpen} experts={resource.agents} />
      <Dialog open={editorOpen} onOpenChange={setEditorOpen}>
        <DialogContent className="fox-agent-editor-dialog">
          <DialogHeader><DialogTitle>{editingAgent ? '管理专家' : '新建专家'}</DialogTitle>{editingAgent && <DialogDescription>{editingAgent.packageSource === 'imported' ? `已安装能力包 ${editingAgent.packageId} · v${editingAgent.packageVersion} · ${editingAgent.packageHash?.slice(0, 12)}…（包内容只读，可复制为本地专家）` : editingAgent.isBuiltin ? `Fox 内置专家 · v${editingAgent.packageVersion}` : `本地可编辑专家 · v${editingAgent.packageVersion}`}</DialogDescription>}</DialogHeader>
          <Tabs className="fox-agent-editor-tabs" value={editorTab} onValueChange={(value) => setEditorTab(value as typeof editorTab)}>
            <TabsList><TabsTrigger value="profile">基本设置</TabsTrigger><TabsTrigger value="resources">能力配置</TabsTrigger>{editingAgent?.packageSource === 'imported' && <TabsTrigger value="versions">版本历史</TabsTrigger>}</TabsList>
            <TabsContent value="profile">
              <div className="fox-agent-editor-grid">
                <div className="fox-agent-editor-profile-row">
                  <label><span>名称</span><Input value={draft.name} disabled={readOnly} onChange={(event) => setDraft((value) => ({ ...value, name: event.target.value }))} /></label>
                  <label><span>分类</span><Select value={draft.category} disabled={readOnly} onValueChange={(category) => setDraft((value) => ({ ...value, category }))}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="general">通用</SelectItem><SelectItem value="engineering">工程</SelectItem><SelectItem value="design">设计</SelectItem><SelectItem value="planning">规划</SelectItem><SelectItem value="review">审查</SelectItem><SelectItem value="business">业务</SelectItem></SelectContent></Select></label>
                  <label><span>默认模型</span><Input value={draft.defaultModel} disabled={readOnly} onChange={(event) => setDraft((value) => ({ ...value, defaultModel: event.target.value }))} /></label>
                </div>
                <div className="fox-agent-editor-icon-field" aria-busy={iconResource.loading}><span>专家头像</span><div className="fox-agent-editor-icon-list">{iconOptions.map((item) => <button key={item.id} type="button" className={draft.icon === item.src ? 'is-selected' : ''} disabled={readOnly} title={item.name} aria-label={item.name} aria-pressed={draft.icon === item.src} onClick={() => setDraft((value) => ({ ...value, icon: item.src }))}><img src={item.src} alt="" /><small>{item.name}</small></button>)}</div></div>
                <div className="fox-agent-editor-copy-grid">
                  <label><span>简介</span><Textarea value={draft.description} disabled={readOnly} onChange={(event) => setDraft((value) => ({ ...value, description: event.target.value }))} /></label>
                  <label><span>开场建议（每行一条，最多 6 条）</span><Textarea className="fox-agent-opening-input" value={draft.openingSuggestions.join('\n')} disabled={readOnly} onChange={(event) => setDraft((value) => ({ ...value, openingSuggestions: event.target.value.split(/\r?\n/).slice(0, 6) }))} /></label>
                </div>
                <label className="is-wide"><span>系统提示词</span><Textarea className="fox-agent-prompt-input" value={draft.systemPrompt} disabled={readOnly} onChange={(event) => setDraft((value) => ({ ...value, systemPrompt: event.target.value }))} /></label>
              </div>
            </TabsContent>
            <TabsContent value="resources" className="fox-agent-editor-resources">
              <section><header><span><Database />知识库</span><b>{selectedResources('knowledge').size} 已选择</b></header><ExpertResourceList items={knowledgeChoices} selected={selectedResources('knowledge')} disabled={readOnly} emptyText={knowledgeResource.loading ? '正在读取知识库…' : knowledgeResource.error || '没有可选择的知识库'} onToggle={(id, checked) => toggleResource('knowledge', id, checked)} /></section>
              <section><header><span><Puzzle />技能</span><b>{selectedResources('skills').size} 已选择</b></header><ExpertResourceList items={skillChoices} selected={selectedResources('skills')} disabled={readOnly} emptyText={skillResource.loading ? '正在扫描 技能…' : skillResource.error || '没有可选择的 技能'} onToggle={(id, checked) => toggleResource('skills', id, checked)} /></section>
              <section><header><span><Globe2 />连接器</span><b>{selectedResources('mcpServers').size} 已选择</b></header><ExpertResourceList items={mcpChoices} selected={selectedResources('mcpServers')} disabled={readOnly} emptyText={mcpResource.loading ? '正在读取 连接器…' : mcpResource.error || '没有可选择的 连接器'} onToggle={(id, checked) => toggleResource('mcpServers', id, checked)} /></section>
              <section><header><span><Wrench />工具权限</span><b>{selectedResources('allowedTools').size} 已选择</b></header><ExpertResourceList items={toolChoices} selected={selectedResources('allowedTools')} disabled={readOnly} emptyText="当前没有可选择的 Runtime 工具" onToggle={(id, checked) => toggleResource('allowedTools', id, checked)} /></section>
            </TabsContent>
            {editingAgent?.packageSource === 'imported' && <TabsContent value="versions" className="fox-agent-editor-resources">
              <section>
                <header><span><History />不可变版本记录</span><b>{packageVersions.length} 个版本</b></header>
                <div className="fox-agent-resource-list">
                  {packageVersions.map((item) => <div key={item.id}><span><History /></span><p><b>v{item.version}<em>{item.status === 'active' ? '当前' : '历史'}</em></b><small>SHA-256 {item.packageHash.slice(0, 16)}… · {new Date(item.activatedAt).toLocaleString('zh-CN')}</small></p>{item.status !== 'active' && <Button variant="outline" size="sm" disabled={packageBusy} onClick={() => void rollbackPackage(item.version)}><RotateCcw size={13} />回滚</Button>}</div>)}
                  {!packageVersions.length && <p className="fox-agent-resource-empty">正在读取版本历史…</p>}
                </div>
              </section>
            </TabsContent>}
          </Tabs>
          <DialogFooter className="fox-agent-editor-actions">
            {editingAgent && !editingAgent.isBuiltin && <Button variant="destructive" disabled={savingAgent} onClick={() => void deleteAgent()}>删除</Button>}
            {editingAgent && <Button variant="outline" disabled={savingAgent} onClick={() => void copyAgent()}>复制</Button>}
            {editingAgent?.runtimeType === 'pi' && <Button variant="outline" disabled={packageBusy} onClick={() => void exportPackage()}><Download size={14} />导出能力包</Button>}
            <span />
            <Button variant="outline" onClick={() => setEditorOpen(false)}>取消</Button>
            {!readOnly && <Button disabled={savingAgent} onClick={() => void saveAgent()}>{savingAgent ? '正在保存' : '保存'}</Button>}
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog open={packagePreviewOpen} onOpenChange={setPackagePreviewOpen}>
        <DialogContent>
          <DialogHeader><DialogTitle>安装能力包前预览</DialogTitle><DialogDescription>安装只登记声明式资源，不会执行包内脚本，也不会自动授予项目权限。</DialogDescription></DialogHeader>
          {packagePreview && <div className="fox-agent-editor-grid">
            <div className="fox-agent-editor-profile-row">
              <label><span>能力包</span><strong>{packagePreview.packageId}</strong></label>
              <label><span>版本变化</span><strong>{packagePreview.currentVersion ? `v${packagePreview.currentVersion} → v${packagePreview.version}` : `安装 v${packagePreview.version}`}</strong></label>
              <label><span>操作</span><strong>{{ install: '安装', upgrade: '升级', no_change: '已是当前版本', downgrade: '拒绝降级', conflict: '版本冲突' }[packagePreview.action]}</strong></label>
            </div>
            <label><span>包哈希</span><code>{packagePreview.packageHash}</code></label>
            <label><span>项目权限请求</span><strong>{packagePreview.requestedProjectPermission}</strong><small>最终权限仍由 Host 和用户在运行时决定。</small></label>
            {(packagePreview.missingResources.agents.length + packagePreview.missingResources.skills.length + packagePreview.missingResources.tools.length + packagePreview.missingResources.mcpServers.length + packagePreview.missingResources.knowledgeReferences.length) > 0 && <div className="fox-page-error">缺少资源：{[
              ...packagePreview.missingResources.agents.map((id) => `Agent ${id}`),
              ...packagePreview.missingResources.skills.map((id) => `技能 ${id}`),
              ...packagePreview.missingResources.tools.map((id) => `Tool ${id}`),
              ...packagePreview.missingResources.mcpServers.map((id) => `MCP ${id}`),
              ...packagePreview.missingResources.knowledgeReferences.map((item) => `Knowledge ${item.id}`),
            ].join('、')}</div>}
            {packagePreview.warnings.map((warning) => <p key={warning} className="fox-page-error">{warning}</p>)}
          </div>}
          <DialogFooter><Button variant="outline" onClick={() => setPackagePreviewOpen(false)}>取消</Button><Button disabled={!packagePreview?.canInstall || packageBusy} onClick={() => void installPackage()}>{packageBusy ? '正在安装' : packagePreview?.action === 'upgrade' ? '确认升级' : packagePreview?.action === 'no_change' ? '无需安装' : '确认安装'}</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    </WorkspacePage>
  )
}

export function AgentDetailPage({ sidebarCollapsed, onSidebar, navigate, agentId, section = 'home' }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace; agentId?: string | null; section?: string }) {
  const resource = useAgents()
  const agent = asCard(resource.agents.find((item) => item.id === agentId) ?? resource.agents[0] ?? fallbackAgent)
  const sectionTitle = section === 'conversations' ? '对话任务' : section === 'memory' ? '记忆' : section === 'skills' ? '技能' : section === 'tools' ? '工具' : '首页'
  return (
    <WorkspacePage className="fox-agent-detail-page" title={agent.name} subtitle={`专家详情 · ${sectionTitle}`} sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} onBack={() => navigate('agents')} actions={<Button className="fox-agent-use-button" size="sm" disabled={!agent.available} onClick={() => navigate('chat', agent.id)}><Sparkles size={14} />召唤专家</Button>}>
      <div className="fox-detail-layout fox-agent-detail-layout">
        <main className="fox-detail-main">
          {section === 'home' && <><section className="fox-agent-hero"><span><img src={agent.image} alt="" /></span><div><div><h1>{agent.name}</h1>{agent.isDefault && <Badge variant="secondary"><i />默认专家</Badge>}</div><p>{agent.description}</p><small>{agent.runtimeType === 'pi' ? 'Fox 原生专家可在知识库服务离线时继续对话、读取和处理已授权项目。' : '系统提示词与管理配置保留在知识库服务，Fox 仅展示普通用户可见和可配置的字段。'}</small></div></section><AgentWorkRecord agentId={agent.id} /><AgentCapabilitiesCard agent={agent} navigate={navigate} /></>}
          {section === 'conversations' && <AgentSectionLayout title="对话任务" description="查看该专家执行过的对话任务、完成状态和关联项目。"><AgentWorkRecord agentId={agent.id} conversationsOnly /></AgentSectionLayout>}
          {section === 'memory' && <AgentSectionLayout title="记忆" description="查看、确认、编辑、启停、处理冲突和删除 Fox Host 管理的长期记忆。"><AgentMemoryManager agent={agent} /></AgentSectionLayout>}
          {section === 'skills' && <AgentSectionLayout title="技能" description="查看和管理为该专家启用的预置能力与用户扩展。"><AgentCapabilitiesCard agent={agent} navigate={navigate} section="skills" /></AgentSectionLayout>}
          {section === 'tools' && <AgentSectionLayout title="工具" description="查看该专家可以使用的 Runtime、CLI 与 连接器工具。"><AgentCapabilitiesCard agent={agent} navigate={navigate} section="tools" /></AgentSectionLayout>}
        </main>
      </div>
    </WorkspacePage>
  )
}
