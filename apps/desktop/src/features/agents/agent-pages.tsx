import { useEffect, useMemo, useState } from 'react'
import { Brain, CalendarDays, CheckCircle2, Clock3, Code2, Folder, Globe2, History, Info, MessageSquareText, Plus, Puzzle, Search, Settings2, Sparkles, Wrench } from 'lucide-react'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { AgentRecord, ConversationSummary } from '@/features/conversations/model/types'
import { AgentCard, type AgentCardData } from '@/features/workspace/entity-card'
import { WorkspacePage } from '@/features/workspace/page-shell'
import type { NavigateWorkspace } from '@/features/workspace/types'
import { useAgents } from './use-agents'
import { useMcpServers } from '@/features/settings/use-mcp-servers'
import { useSkills } from '@/features/settings/use-skills'

function asCard(agent: AgentRecord): AgentCardData {
  return { ...agent, image: agent.icon || (agent.runtimeType === 'pi' ? '/mascot/fox_magic.png' : '/mascot/fox_search.png'), tools: agent.resources.tools.length, knowledge: agent.resources.knowledges.length, mcps: agent.resources.mcps.length, skills: agent.resources.skills.length, recent: agent.runtimeType === 'pi' ? '本机' : '知识库', active: agent.isDefault }
}

const fallbackAgent: AgentRecord = { id: 'fox-general', name: 'Fox 通用助手', description: 'Fox 默认通用智能体', runtimeType: 'pi', defaultModel: 'configured-model', icon: null, capabilities: ['files', 'tools', 'reasoning'], resources: { tools: [{ id: 'files', name: '文件读写', description: '读取和处理已授权项目文件' }, { id: 'tools', name: '本地工具', description: '调用 Fox Runtime 提供的本地工具' }, { id: 'reasoning', name: '任务推理', description: '规划并执行多步骤任务' }], knowledges: [], mcps: [], skills: [] }, configurableItems: {}, isDefault: true, available: true }

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
            <TabsTrigger value="scheduled"><Clock3 />自动任务</TabsTrigger>
          </TabsList>
        </div>
        <TabsContent value="timeline" className="fox-agent-work-content">
          <div className="fox-agent-work-metrics">
            <div><strong>{record.activeDays}<small>天</small></strong><span>活跃天数 <Info /></span></div>
            <div><strong>0</strong><span>自动任务 <Info /></span></div>
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
        <TabsContent value="scheduled" className="fox-agent-work-empty"><Clock3 /><p>当前阶段尚未启用自动任务。</p></TabsContent>
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
  { id: 'list_mcp_tools', name: 'list_mcp_tools', description: '读取当前已启用 MCP 服务提供的工具清单', kind: 'MCP' },
  { id: 'call_mcp_tool', name: 'call_mcp_tool', description: '通过 Fox 权限审批调用指定的 MCP 工具', kind: 'MCP' },
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
  list_mcp_tools: '列出 MCP 服务提供的工具',
  call_mcp_tool: '调用 MCP 服务中的工具',
}

function AgentCapabilitiesCard({ agent, navigate, section = 'all' }: { agent: AgentCardData; navigate: NavigateWorkspace; section?: 'all' | 'skills' | 'tools' }) {
  const skillResource = useSkills(agent.id)
  const mcpResource = useMcpServers()
  const [managerOpen, setManagerOpen] = useState(false)
  const [managerTab, setManagerTab] = useState<'skills' | 'tools'>('skills')
  const [draftSkills, setDraftSkills] = useState<Set<string>>(new Set())
  const [draftMcps, setDraftMcps] = useState<Set<string>>(new Set())
  const [saving, setSaving] = useState(false)
  const localAgent = agent.runtimeType === 'pi'
  const defaultSkillIds = new Set(agent.resources.skills.map((item) => item.id))
  const defaultSkills = agent.resources.skills
  const userSkills = localAgent ? skillResource.items.filter((item) => !defaultSkillIds.has(item.id)) : []
  const skills = localAgent
    ? [...defaultSkills, ...userSkills.filter((item) => item.enabled).map((item) => ({ id: item.id, name: item.name, description: item.description }))]
    : agent.resources.skills
  const enabledMcps = mcpResource.items.filter((item) => item.enabled)
  const tools = agent.runtimeType === 'pi'
    ? [...foxRuntimeTools, ...enabledMcps.map((item) => ({ id: item.id, name: item.name, description: item.command, kind: 'MCP' }))]
    : [
        ...agent.resources.tools.map((item) => ({ ...item, kind: 'Tool' })),
        ...agent.resources.mcps.map((item) => ({ ...item, kind: 'MCP' })),
      ]

  const openManager = (tab: 'skills' | 'tools') => {
    setManagerTab(tab)
    setDraftSkills(new Set(userSkills.filter((item) => item.enabled).map((item) => item.id)))
    setDraftMcps(new Set(mcpResource.items.filter((item) => item.enabled).map((item) => item.id)))
    setManagerOpen(true)
  }

  const manageResources = (tab: 'skills' | 'tools') => {
    if (section === 'all') {
      navigate('agent-detail', agent.id, { documentId: tab })
      return
    }
    openManager(tab)
  }

  const toggleDraft = (kind: 'skills' | 'tools', id: string, enabled: boolean) => {
    const update = (current: Set<string>) => {
      const next = new Set(current)
      if (enabled) next.add(id)
      else next.delete(id)
      return next
    }
    if (kind === 'skills') setDraftSkills(update)
    else setDraftMcps(update)
  }

  const saveResources = async () => {
    if (!localAgent) return
    setSaving(true)
    const skillChanges = userSkills.filter((item) => draftSkills.has(item.id) !== item.enabled)
    const mcpChanges = mcpResource.items.filter((item) => draftMcps.has(item.id) !== item.enabled)
    const results = await Promise.all([
      ...skillChanges.map((item) => skillResource.setEnabled(item.id, draftSkills.has(item.id))),
      ...mcpChanges.map((item) => mcpResource.setEnabled(item.id, draftMcps.has(item.id))),
    ])
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
        <h2>{section === 'skills' ? 'Skills' : section === 'tools' ? '工具' : '能力与工具'}</h2>
        {section !== 'tools' && <section>
          <header><h3>能力（{skills.length}）</h3><Button variant="outline" size="sm" onClick={() => manageResources('skills')}><Settings2 />管理</Button></header>
          <div className="fox-agent-capability-grid">{skills.length ? skills.map((skill) => <div key={skill.id}><span><Puzzle /></span><p><b>{skill.name || skill.id}</b><small>{skill.description || `Skill · ${skill.id}`}</small></p><Badge variant="outline">Skill</Badge></div>) : <p className="fox-agent-capability-empty">此智能体暂未配置 Skills。</p>}</div>
        </section>}
        {section !== 'skills' && <section>
          <header><h3>工具（{tools.length}）</h3><Button variant="outline" size="sm" onClick={() => manageResources('tools')}><Settings2 />管理</Button></header>
          <div className="fox-agent-capability-grid">{tools.length ? tools.slice(0, 16).map((tool) => {
            const toolName = tool.id || tool.name
            const description = tool.description || toolDescriptions[toolName] || `调用 ${toolName} 工具`
            return <div key={`${tool.kind}:${toolName}`}><span>{tool.kind === 'CLI' ? <Code2 /> : tool.kind === 'MCP' ? <Globe2 /> : /file|read|write|edit/i.test(toolName) ? <Wrench /> : <Search />}</span><p><b><code>{toolName}</code></b><small>{description}</small></p><Badge variant="outline">{tool.kind}</Badge></div>
          }) : <p className="fox-agent-capability-empty">当前 Runtime 尚未返回 Tool、CLI 或 MCP 工具。</p>}</div>
        </section>}
      </Card>
      <Dialog open={managerOpen} onOpenChange={setManagerOpen}>
        <DialogContent className="fox-agent-resource-dialog">
          <DialogHeader>
            <div className="fox-agent-resource-dialog-title"><div><DialogTitle>管理能力与工具</DialogTitle><DialogDescription>{localAgent ? `预置资源默认开启且不可修改，用户添加的资源可以自由启用或停用。` : '知识库智能体的能力与工具由知识库服务统一管理。'}</DialogDescription></div>{localAgent && <Button variant="outline" size="sm" onClick={() => { setManagerOpen(false); navigate(managerTab === 'skills' ? 'skills' : 'mcp') }}><Plus />{managerTab === 'skills' ? '添加 Skill' : '添加工具'}</Button>}</div>
          </DialogHeader>
          <Tabs value={managerTab} onValueChange={(value) => setManagerTab(value as 'skills' | 'tools')}>
            <TabsList><TabsTrigger value="skills">能力（{localAgent ? defaultSkills.length + draftSkills.size : skills.length}）</TabsTrigger><TabsTrigger value="tools">工具（{localAgent ? foxRuntimeTools.length + draftMcps.size : tools.length}）</TabsTrigger></TabsList>
            <TabsContent value="skills" className="fox-agent-resource-list">
              {localAgent ? <>{defaultSkills.map((skill) => <div key={`default:${skill.id}`}><span><Puzzle /></span><p><b>{skill.name || skill.id}<em>预置</em></b><small>{skill.description || `Skill · ${skill.id}`}</small></p><Switch checked disabled aria-label={`${skill.name} 默认开启`} /></div>)}{userSkills.map((skill) => <div key={skill.id}><span><Puzzle /></span><p><b>{skill.name}<em>用户添加</em></b><small>{skill.description || skill.id}</small></p><Switch checked={draftSkills.has(skill.id)} disabled={!skill.valid || saving} onCheckedChange={(checked) => toggleDraft('skills', skill.id, checked)} /></div>)}{!defaultSkills.length && !userSkills.length && <p className="fox-agent-resource-empty">还没有可用的 Skill，可通过右上角“添加 Skill”安装。</p>}</> : skills.length ? skills.map((skill) => <div key={skill.id}><span><Puzzle /></span><p><b>{skill.name}<em>预置</em></b><small>{skill.description}</small></p><Switch checked disabled aria-label={`${skill.name} 默认开启`} /></div>) : <p className="fox-agent-resource-empty">该智能体没有公开的能力信息。</p>}
            </TabsContent>
            <TabsContent value="tools" className="fox-agent-resource-list">
              {localAgent ? <>{foxRuntimeTools.map((tool) => <div key={tool.id}><span>{tool.kind === 'CLI' ? <Code2 /> : <Wrench />}</span><p><b><code>{tool.id}</code><em>预置</em></b><small>{tool.description}</small></p><Switch checked disabled aria-label={`${tool.id} 默认开启`} /></div>)}{mcpResource.items.map((server) => <div key={server.id}><span><Globe2 /></span><p><b>{server.name}<em>用户添加</em></b><small>MCP · {server.command}</small></p><Switch checked={draftMcps.has(server.id)} disabled={saving} onCheckedChange={(checked) => toggleDraft('tools', server.id, checked)} /></div>)}</> : tools.length ? tools.map((tool) => <div key={`${tool.kind}:${tool.id}`}><span><Wrench /></span><p><b><code>{tool.id}</code><em>预置</em></b><small>{tool.description}</small></p><Switch checked disabled aria-label={`${tool.id} 默认开启`} /></div>) : <p className="fox-agent-resource-empty">该智能体没有公开的工具信息。</p>}
            </TabsContent>
          </Tabs>
          <DialogFooter><Button variant="outline" onClick={() => setManagerOpen(false)}>取消</Button><Button disabled={!localAgent || saving} onClick={() => void saveResources()}>{saving ? '正在保存' : '保存更改'}</Button></DialogFooter>
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
  const cards = (resource.agents.length ? resource.agents : [fallbackAgent]).map(asCard)
  const [query, setQuery] = useState('')
  const [availability, setAvailability] = useState<'all' | 'available' | 'unavailable'>('all')
  const [environment, setEnvironment] = useState<'all' | 'local' | 'knowledge'>('all')
  const normalizedQuery = query.trim().toLocaleLowerCase()
  const filteredCards = cards.filter((agent) => {
    if (availability === 'available' && !agent.available) return false
    if (availability === 'unavailable' && agent.available) return false
    if (environment === 'local' && agent.runtimeType !== 'pi') return false
    if (environment === 'knowledge' && agent.runtimeType === 'pi') return false
    return !normalizedQuery || [agent.name, agent.description, agent.defaultModel].some((value) => value.toLocaleLowerCase().includes(normalizedQuery))
  })
  return (
    <WorkspacePage title="智能体" subtitle="查看并选择本地与知识库服务中的智能体" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
      <section className="fox-page-content fox-agent-list-page">
        <div className="fox-page-intro"><div><span>FOX AGENTS</span><h1>选择一个智能体</h1><p>一个会话始终绑定一个智能体。知识库服务离线时，Fox 原生 Agent 仍可使用。</p></div><div className="fox-page-metrics"><span><b>{cards.length}</b><small>智能体</small></span><span><b>{cards.filter((item) => item.available).length}</b><small>当前可用</small></span><span><b>{cards.reduce((sum, item) => sum + item.tools, 0)}</b><small>能力项</small></span></div></div>
        <div className="fox-page-toolbar fox-agent-toolbar">
          <Select value={availability} onValueChange={(value) => setAvailability(value as typeof availability)}><SelectTrigger className="fox-agent-filter"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">全部状态</SelectItem><SelectItem value="available">可使用</SelectItem><SelectItem value="unavailable">不可用</SelectItem></SelectContent></Select>
          <Select value={environment} onValueChange={(value) => setEnvironment(value as typeof environment)}><SelectTrigger className="fox-agent-filter"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">全部环境</SelectItem><SelectItem value="local">本机</SelectItem><SelectItem value="knowledge">知识库</SelectItem></SelectContent></Select>
          <label><Search size={14} /><Input value={query} onChange={(event) => setQuery(event.target.value)} aria-label="搜索智能体" placeholder="搜索智能体..." /></label>
          <span className="fox-agent-result-count">{filteredCards.length} 个结果</span>
          <Button variant="ghost" size="sm" onClick={() => void resource.refresh()}>刷新</Button>
        </div>
        {resource.error && <p className="fox-page-error">{resource.error}</p>}
        {filteredCards.length ? <div className="fox-entity-grid fox-shadcn-entity-grid">{filteredCards.map((agent) => <AgentCard key={agent.id} agent={agent} onOpen={() => navigate('agent-detail', agent.id)} onUse={() => navigate('chat', agent.id)} />)}</div> : <p className="fox-page-empty">没有符合当前筛选条件的智能体。</p>}
      </section>
    </WorkspacePage>
  )
}

export function AgentDetailPage({ sidebarCollapsed, onSidebar, navigate, agentId, section = 'home' }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace; agentId?: string | null; section?: string }) {
  const resource = useAgents()
  const agent = asCard(resource.agents.find((item) => item.id === agentId) ?? resource.agents[0] ?? fallbackAgent)
  const sectionTitle = section === 'conversations' ? '对话任务' : section === 'memory' ? '记忆' : section === 'skills' ? 'Skills' : section === 'tools' ? '工具' : '首页'
  return (
    <WorkspacePage className="fox-agent-detail-page" title={agent.name} subtitle={`智能体详情 · ${sectionTitle}`} sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} onBack={() => navigate('agents')} actions={<Button size="sm" disabled={!agent.available} onClick={() => navigate('chat', agent.id)}><Sparkles size={14} />使用此智能体</Button>}>
      <div className="fox-detail-layout fox-agent-detail-layout">
        <main className="fox-detail-main">
          {section === 'home' && <><section className="fox-agent-hero"><span><img src={agent.image} alt="" /></span><div><div><h1>{agent.name}</h1>{agent.isDefault && <Badge variant="secondary"><i />默认智能体</Badge>}</div><p>{agent.description}</p><small>{agent.runtimeType === 'pi' ? 'Fox 原生 Agent 可在知识库服务离线时继续对话、读取和处理已授权项目。' : '系统提示词与管理配置保留在知识库服务，Fox 仅展示普通用户可见和可配置的字段。'}</small></div></section><AgentWorkRecord agentId={agent.id} /><AgentCapabilitiesCard agent={agent} navigate={navigate} /></>}
          {section === 'conversations' && <AgentSectionLayout title="对话任务" description="查看该智能体执行过的对话任务、完成状态和关联项目。"><AgentWorkRecord agentId={agent.id} conversationsOnly /></AgentSectionLayout>}
          {section === 'memory' && <AgentSectionLayout title="记忆" description="管理该智能体保存的长期记忆、用户偏好和项目上下文。"><Card className="fox-agent-section-card fox-agent-memory-empty"><span><Brain /></span><h2>记忆尚未启用</h2><p>后续将支持查看和管理该智能体保存的长期记忆、用户偏好与项目上下文。</p></Card></AgentSectionLayout>}
          {section === 'skills' && <AgentSectionLayout title="Skills" description="查看和管理为该智能体启用的预置能力与用户扩展。"><AgentCapabilitiesCard agent={agent} navigate={navigate} section="skills" /></AgentSectionLayout>}
          {section === 'tools' && <AgentSectionLayout title="工具" description="查看该智能体可以使用的 Runtime、CLI 与 MCP 工具。"><AgentCapabilitiesCard agent={agent} navigate={navigate} section="tools" /></AgentSectionLayout>}
        </main>
      </div>
    </WorkspacePage>
  )
}
