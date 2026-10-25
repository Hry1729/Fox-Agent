import { useEffect, useMemo, useState } from 'react'
import { Archive, Bot, ChevronRight, ExternalLink, FolderOpen, Info, LoaderCircle, MoreHorizontal, Plus, RotateCcw } from 'lucide-react'
import { notify as toast } from '@/features/notifications'
import { cn } from '@/lib/utils'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, runAfterMenuClose } from '@/components/ui/dropdown-menu'
import { WorkspacePage } from '@/features/workspace/page-shell'
import type { NavigateWorkspace } from '@/features/workspace/types'
import type { SettingsSectionAnchor } from '@/features/workspace/management-navigation'
import { desktopClient, desktopErrorDetails, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { AgentRecord, ProjectManagementRecord, ProjectRecord } from '@/features/conversations/model/types'
import { useAgents } from '@/features/agents/use-agents'
import { expertCatalogAgents } from '@/features/agents/agent-classification'
import { asAgentCard } from '@/features/agents/agent-card-data'
import { AgentEditorProfileFields, AgentEditorResourcesFields, useAgentEditor } from '@/features/agents/agent-pages'
import { AgentBackendSettingsSection } from './agent-backend-settings'
import { useModelProviders } from './use-model-providers'
import { useModelService } from './use-model-service'
import { SettingsGroup, SettingsPageLayout, SettingsRow, settingsSectionId, useScrollToSettingsSection } from './settings-layout'
import { rememberPluginReturn, serviceStatusLabel, useStoredPreference } from './settings-shared'

type PermissionMode = ProjectRecord['permissionMode']

const PERMISSION_LABELS: Record<PermissionMode, string> = { read_only: '只读', ask: '询问', allow: '允许' }

function PermissionSelect({ value, disabled, label, onChange, className }: { value: PermissionMode; disabled?: boolean; label: string; onChange: (value: PermissionMode) => void; className?: string }) {
  return <Select value={value} disabled={disabled} onValueChange={(next) => onChange(next as PermissionMode)}>
    <SelectTrigger className={cn('fox-settings-compact-select', className)} aria-label={label}><SelectValue /></SelectTrigger>
    <SelectContent><SelectItem value="read_only">只读</SelectItem><SelectItem value="ask">询问</SelectItem><SelectItem value="allow">允许</SelectItem></SelectContent>
  </Select>
}

function projectStorageLabel(bytes: number | null) {
  if (bytes === null) return '未统计'
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`
}

function expertSourceLabel(agent: AgentRecord) {
  if (agent.packageSource === 'imported') return `专家包 v${agent.packageVersion}`
  if (agent.packageSource === 'builtin') return '内置专家'
  if (agent.runtimeType === 'pi') return '本机专家'
  return '资料库服务专家'
}

type GeneralAssistantDraft = { name: string; description: string; systemPrompt: string; openingSuggestions: string }

/** Fox 通用助手: same fields and save call as before, now edited in the 480px Sheet. */
function useGeneralAssistant() {
  const [assistant, setAssistant] = useState<AgentRecord | null>(null)
  const [loading, setLoading] = useState(desktopRuntimeAvailable)
  const [saving, setSaving] = useState(false)
  useEffect(() => {
    let cancelled = false
    if (!desktopRuntimeAvailable) { setLoading(false); return }
    void desktopClient.listAgents()
      .then((items) => { if (!cancelled) setAssistant(items.find((item) => item.id === 'fox-general') ?? null) })
      .catch((cause) => { if (!cancelled) toast.error('通用助手配置读取失败', { description: desktopErrorDetails(cause).message }) })
      .finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [])
  const save = async (draft: GeneralAssistantDraft) => {
    if (!assistant) return false
    if (!draft.name.trim() || !draft.systemPrompt.trim()) { toast.error('请填写助手名称和工作要求'); return false }
    setSaving(true)
    try {
      const updated = await desktopClient.saveAgent({
        id: assistant.id,
        name: draft.name.trim(),
        description: draft.description.trim(),
        icon: assistant.icon,
        category: assistant.category,
        systemPrompt: draft.systemPrompt.trim(),
        defaultModel: assistant.defaultModel,
        openingSuggestions: draft.openingSuggestions.split(/\r?\n/).map((item) => item.trim()).filter(Boolean).slice(0, 6),
        packageManifest: assistant.packageManifest,
      })
      setAssistant(updated)
      toast.success('Fox 通用助手设置已保存')
      return true
    } catch (cause) {
      toast.error('保存通用助手设置失败', { description: desktopErrorDetails(cause).message })
      return false
    } finally {
      setSaving(false)
    }
  }
  return { assistant, loading, saving, save }
}

function useManagedProjects() {
  const [projects, setProjects] = useState<ProjectManagementRecord[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  const load = async () => {
    setLoading(true)
    try { setProjects(await desktopClient.listManagedProjects()); setError(null) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLoading(false) }
  }
  useEffect(() => { void load() }, [])
  const updatePermission = async (project: ProjectManagementRecord, permissionMode: PermissionMode) => {
    setBusyId(project.id)
    try {
      const updated = await desktopClient.updateProjectPermission(project.id, permissionMode)
      setProjects((items) => items.map((item) => item.id === updated.id ? { ...item, permissionMode: updated.permissionMode, updatedAt: updated.updatedAt } : item))
      toast.success(`已更新 ${updated.name} 的执行方式`)
    } catch (cause) {
      toast.error('权限更新失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally { setBusyId(null) }
  }
  const choosePath = async (project: ProjectManagementRecord) => {
    setBusyId(project.id)
    try {
      const rootPath = await desktopClient.pickProjectFolder()
      if (!rootPath) return
      const updated = await desktopClient.updateProjectPath(project.id, rootPath)
      setProjects((items) => items.map((item) => item.id === updated.id ? updated : item))
      toast.success(`已迁移 ${updated.name} 的项目路径`)
    } catch (cause) {
      toast.error('项目路径更新失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally { setBusyId(null) }
  }
  const setArchived = async (project: ProjectManagementRecord, archived: boolean) => {
    setBusyId(project.id)
    try {
      await desktopClient.setProjectArchived(project.id, archived)
      setProjects((items) => items.map((item) => item.id === project.id ? { ...item, archivedAt: archived ? Date.now() : null } : item))
      toast.success(archived ? `已归档 ${project.name}` : `已恢复 ${project.name}`)
    } catch (cause) {
      toast.error(archived ? '项目归档失败' : '项目恢复失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally { setBusyId(null) }
  }
  const openRoot = async (project: ProjectManagementRecord) => {
    try { await desktopClient.openProjectRoot(project.id) }
    catch (cause) { toast.error('无法打开项目目录', { description: cause instanceof Error ? cause.message : String(cause) }) }
  }
  return { projects, loading, error, busyId, load, updatePermission, choosePath, setArchived, openRoot }
}

/** Expert edit Sheet: 基本 / 能力, the same editor (fields, manifest, save) as the expert management page. */
function ExpertEditSheet({ agent, onClose, onSaved, navigate }: { agent: AgentRecord | null; onClose: () => void; onSaved: () => void; navigate: NavigateWorkspace }) {
  const editor = useAgentEditor(Boolean(agent))
  const [tab, setTab] = useState<'profile' | 'resources'>('profile')
  useEffect(() => {
    if (agent) { editor.reset(agent); setTab('profile') }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agent?.id])
  const card = agent ? asAgentCard(agent) : null
  const save = async () => {
    const saved = await editor.save()
    if (saved) { onSaved(); onClose() }
  }
  const openPlugins = (view: 'skills' | 'mcp') => {
    if (!agent) return
    rememberPluginReturn({ view: 'settings-ai', entityId: agent.id, label: agent.name })
    navigate(view)
  }
  const copy = async () => {
    const copied = await editor.copy()
    if (copied) { onSaved(); navigate('settings-ai', copied.id) }
  }
  const readOnlyNote = agent && editor.readOnly
    ? agent.runtimeType !== 'pi' ? '资料库服务专家的设置由服务端统一管理，这里只能查看。' : agent.packageSource === 'imported' ? '已安装专家包的内容只读，复制为本机专家后可以修改。' : '内置专家的资料只读，复制为本机专家后可以修改。'
    : null
  return <Sheet open={Boolean(agent)} onOpenChange={(open) => { if (!open) onClose() }}>
    <SheetContent side="right" className="fox-settings-sheet fox-settings-expert-sheet">
      {agent && card && <>
        <SheetHeader className="fox-settings-sheet-head">
          <span className="fox-settings-provider-mark is-large is-avatar"><img src={card.image} alt="" /></span>
          <div className="fox-settings-sheet-title">
            <SheetTitle><span>{editor.draft.name || agent.name}</span>{agent.isDefault && <Badge variant="secondary" className="fox-settings-pill">默认</Badge>}</SheetTitle>
            <SheetDescription>{expertSourceLabel(agent)}</SheetDescription>
          </div>
        </SheetHeader>
        <div className="fox-settings-sheet-body">
          <Tabs className="fox-settings-segmented fox-settings-sheet-tabs" value={tab} onValueChange={(value) => { if (value === 'profile' || value === 'resources') setTab(value) }}>
            <TabsList aria-label="专家设置"><TabsTrigger value="profile">基本</TabsTrigger><TabsTrigger value="resources">能力</TabsTrigger></TabsList>
          </Tabs>
          {readOnlyNote && <p className="fox-settings-note"><Info />{readOnlyNote}</p>}
          {tab === 'profile' ? <AgentEditorProfileFields editor={editor} /> : <>
            {agent.runtimeType === 'pi' && <div className="fox-settings-sheet-actions"><Button type="button" variant="outline" size="sm" onClick={() => openPlugins('skills')}><Plus />添加技能</Button><Button type="button" variant="outline" size="sm" onClick={() => openPlugins('mcp')}><Plus />添加连接器</Button></div>}
            <AgentEditorResourcesFields editor={editor} />
          </>}
        </div>
        <div className="fox-settings-sheet-foot">
          <Button type="button" variant="ghost" className="fox-settings-sheet-link" onClick={() => navigate('agent-detail', agent.id, { documentId: 'home' })}>记忆与工作记录<ChevronRight /></Button>
          <span className="is-grow" />
          {editor.readOnly
            ? agent.runtimeType === 'pi' && <Button type="button" variant="outline" disabled={editor.saving} onClick={() => void copy()}>复制为本机专家</Button>
            : <><Button type="button" variant="outline" onClick={onClose}>取消</Button><Button type="button" disabled={editor.saving || !editor.draft.name.trim() || !editor.draft.systemPrompt.trim()} onClick={() => void save()}>{editor.saving && <LoaderCircle className="animate-spin" />}保存</Button></>}
        </div>
      </>}
    </SheetContent>
  </Sheet>
}

export function AgentSettingsPage({ sidebarCollapsed, onSidebar, navigate, section, agentId }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace; section?: SettingsSectionAnchor; agentId?: string }) {
  useScrollToSettingsSection(section)
  const model = useModelService()
  const providers = useModelProviders()
  const general = useGeneralAssistant()
  const agents = useAgents()
  const projectsResource = useManagedProjects()
  const [defaultPermission, setDefaultPermission] = useStoredPreference<PermissionMode>('fox.preferences.defaultPermission', 'ask')
  const [assistantDraft, setAssistantDraft] = useState<GeneralAssistantDraft | null>(null)
  const [editingExpertId, setEditingExpertId] = useState<string | null>(agentId ?? null)
  const [projectId, setProjectId] = useState<string | null>(null)
  useEffect(() => { if (agentId) setEditingExpertId(agentId) }, [agentId])

  const experts = useMemo(() => expertCatalogAgents(agents.agents), [agents.agents])
  const editingExpert = editingExpertId ? agents.agents.find((item) => item.id === editingExpertId) ?? null : null
  const selected = providers.items.flatMap((provider) => provider.models.map((item) => ({ provider, item }))).find(({ provider, item }) => provider.isDefault && item.isDefault)
  const selectModel = async (value: string) => {
    const [providerId, modelId] = value.split('::')
    const provider = providers.items.find((item) => item.id === providerId)
    if (!provider) return
    const saved = await providers.save({ id: provider.id, name: provider.name, icon: provider.icon, baseUrl: provider.baseUrl, apiType: provider.apiType, isDefault: true, models: provider.models.map((item) => ({ ...item, isDefault: item.modelId === modelId })) })
    if (saved) { await model.refresh(); toast.success('默认模型已更新') }
  }
  const openAssistant = () => {
    const assistant = general.assistant
    if (!assistant) return
    setAssistantDraft({ name: assistant.name, description: assistant.description, systemPrompt: assistant.systemPrompt, openingSuggestions: assistant.openingSuggestions.join('\n') })
  }
  const saveAssistant = async () => {
    if (assistantDraft && await general.save(assistantDraft)) setAssistantDraft(null)
  }
  const { projects, busyId } = projectsResource
  const activeProjects = projects.filter((project) => project.archivedAt === null)
  const archivedProjects = projects.filter((project) => project.archivedAt !== null)
  const openProject = projectId ? projects.find((item) => item.id === projectId) ?? null : null
  const currentModel = model.service
    ? `当前 ${model.service.name} · ${serviceStatusLabel(model.service.lastStatus)} · ${model.service.contextWindow.toLocaleString()} tokens · ${model.service.supportsImageInput ? '支持图片' : '仅文本'}`
    : '新对话和专家未指定模型时使用'

  return <WorkspacePage title="Agent" subtitle="助手、专家与项目权限" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
    <SettingsPageLayout title="Agent" description="配置通用助手、专家、新会话使用的 Agent 和项目权限。">
      <SettingsGroup title="通用助手">
        <div className="fox-settings-providers">
          <div className="fox-settings-provider">
            <button type="button" className="fox-settings-provider-main" disabled={!general.assistant} onClick={openAssistant} aria-haspopup="dialog" aria-label="编辑 Fox 通用助手">
              <span className="fox-settings-provider-mark is-avatar">{general.assistant?.icon ? <img src={general.assistant.icon} alt="" /> : general.loading ? <LoaderCircle className="animate-spin" /> : <Bot />}</span>
              <span className="fox-settings-provider-text"><span className="fox-settings-provider-name"><b>{general.assistant?.name ?? (general.loading ? '正在读取通用助手…' : '暂时无法读取通用助手')}</b></span><small>{general.assistant?.description || '名称、简介、工作要求和新对话开场建议'}</small></span>
              <ChevronRight className="fox-settings-provider-chevron" />
            </button>
          </div>
        </div>
      </SettingsGroup>

      <SettingsGroup title="默认">
        <SettingsRow title="默认模型" description={currentModel}>
          {providers.items.some((provider) => provider.models.length)
            ? <Select value={selected ? `${selected.provider.id}::${selected.item.modelId}` : undefined} disabled={providers.busyId !== null} onValueChange={(value) => void selectModel(value)}>
              <SelectTrigger className="fox-settings-default-model" aria-label="默认模型"><SelectValue placeholder="选择默认模型" /></SelectTrigger>
              <SelectContent>{providers.items.flatMap((provider) => provider.models.map((item) => <SelectItem key={`${provider.id}:${item.id}`} value={`${provider.id}::${item.modelId}`}>{provider.name} · {item.displayName || item.modelId}</SelectItem>))}</SelectContent>
            </Select>
            : <Button variant="outline" size="sm" onClick={() => navigate('settings-models')}>添加模型服务</Button>}
        </SettingsRow>
      </SettingsGroup>

      {desktopRuntimeAvailable && <AgentBackendSettingsSection client={desktopClient} />}

      <SettingsGroup title="专家" aside={<>{experts.length > 0 && <span>{experts.length} 个专家</span>}<Button type="button" variant="ghost" size="sm" onClick={() => navigate('agents')}>专家中心<ChevronRight /></Button></>}>
        {agents.loading && experts.length === 0 && <p className="fox-settings-empty"><LoaderCircle className="animate-spin" /> 正在读取专家…</p>}
        {!agents.loading && experts.length === 0 && <p className="fox-settings-empty">{agents.error ? `专家读取失败：${agents.error}` : '还没有专家，可在专家中心新建或导入。'}</p>}
        <div className="fox-settings-providers">
          {experts.map((agent) => {
            const card = asAgentCard(agent)
            const counts = [card.skills ? `技能 ${card.skills}` : '', card.knowledge ? `资料库 ${card.knowledge}` : '', card.mcps ? `连接器 ${card.mcps}` : ''].filter(Boolean).join(' · ')
            return <div className={cn('fox-settings-provider', editingExpertId === agent.id && 'is-open')} key={agent.id}>
              <button type="button" className="fox-settings-provider-main" onClick={() => setEditingExpertId(agent.id)} aria-haspopup="dialog" aria-label={`编辑 ${agent.name}`}>
                <span className="fox-settings-provider-mark is-avatar"><img src={card.image} alt="" /></span>
                <span className="fox-settings-provider-text"><span className="fox-settings-provider-name"><b>{agent.name}</b>{agent.isDefault && <Badge variant="secondary" className="fox-settings-pill">默认</Badge>}</span><small title={agent.description}>{agent.description || expertSourceLabel(agent)}</small></span>
                <span className="fox-settings-provider-status"><i className={cn('fox-settings-dot', agent.available ? 'is-ok' : 'is-empty')} aria-hidden="true" /><b>{expertSourceLabel(agent)}</b>{counts && <><span className="is-separator">·</span><span>{counts}</span></>}</span>
              </button>
            </div>
          })}
        </div>
      </SettingsGroup>

      <SettingsGroup id={settingsSectionId('projects')} title="项目权限" aside={<Button type="button" variant="ghost" size="sm" disabled={projectsResource.loading} onClick={() => void projectsResource.load()}>{projectsResource.loading ? <LoaderCircle className="animate-spin" /> : <RotateCcw />}刷新</Button>}>
        <SettingsRow title="新项目权限" description="只影响之后加入的新项目；建议「询问」，写入或运行命令前先确认">
          <PermissionSelect value={defaultPermission} label="新项目权限" onChange={setDefaultPermission} />
        </SettingsRow>
        {projectsResource.error
          ? <div className="fox-settings-inline-alert" role="alert"><span><b>暂时无法读取项目列表</b><small title={projectsResource.error}>{projectsResource.error}</small></span><Button variant="outline" size="sm" onClick={() => void projectsResource.load()}>重试</Button></div>
          : projectsResource.loading && projects.length === 0
            ? <p className="fox-settings-empty"><LoaderCircle className="animate-spin" /> 正在检查目录、Git 状态与占用空间…</p>
            : activeProjects.length === 0 && <p className="fox-settings-empty">还没有项目，可从主界面的「添加项目」选择一个本机文件夹。</p>}
        <div className="fox-settings-providers">
          {activeProjects.map((project) => <div className={cn('fox-settings-provider', projectId === project.id && 'is-open')} key={project.id} data-project-id={project.id}>
            <button type="button" className="fox-settings-provider-main" onClick={() => setProjectId(project.id)} aria-haspopup="dialog" aria-label={`查看 ${project.name}`}>
              <span className="fox-settings-provider-mark"><FolderOpen /></span>
              <span className="fox-settings-provider-text"><span className="fox-settings-provider-name"><b>{project.name}</b>{!project.pathExists && <Badge variant="destructive" className="fox-settings-pill">路径失效</Badge>}</span><small title={project.rootPath}>{project.rootPath}</small></span>
            </button>
            <PermissionSelect value={project.permissionMode} disabled={busyId === project.id} label={`${project.name} 执行方式`} onChange={(value) => void projectsResource.updatePermission(project, value)} />
            <DropdownMenu modal={false}>
              <DropdownMenuTrigger asChild><Button variant="ghost" size="icon-sm" className="fox-settings-provider-more" aria-label={`管理 ${project.name}`}><MoreHorizontal /></Button></DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem disabled={!project.pathExists} onSelect={() => void projectsResource.openRoot(project)}><ExternalLink />打开目录</DropdownMenuItem>
                <DropdownMenuItem disabled={busyId === project.id} onSelect={() => runAfterMenuClose(() => void projectsResource.choosePath(project))}><FolderOpen />迁移目录</DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem disabled={busyId === project.id} onSelect={() => void projectsResource.setArchived(project, true)}><Archive />归档</DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>)}
        </div>
      </SettingsGroup>

      {archivedProjects.length > 0 && <SettingsGroup title="已归档项目" aside={`${archivedProjects.length} 个`}>
        {archivedProjects.map((project) => <SettingsRow key={project.id} title={project.name} description={<span title={project.rootPath}>{project.rootPath}</span>}>
          <Button variant="outline" size="sm" disabled={busyId === project.id} onClick={() => void projectsResource.setArchived(project, false)}><RotateCcw />取消归档</Button>
        </SettingsRow>)}
      </SettingsGroup>}
    </SettingsPageLayout>

    <Sheet open={assistantDraft !== null} onOpenChange={(open) => { if (!open) setAssistantDraft(null) }}>
      <SheetContent side="right" className="fox-settings-sheet">
        {assistantDraft && <>
          <SheetHeader className="fox-settings-sheet-head">
            <span className="fox-settings-provider-mark is-large is-avatar">{general.assistant?.icon ? <img src={general.assistant.icon} alt="" /> : <Bot />}</span>
            <div className="fox-settings-sheet-title"><SheetTitle><span>Fox 通用助手</span></SheetTitle><SheetDescription>应用于之后新建的通用助手对话，不改变专家和远程助手。</SheetDescription></div>
          </SheetHeader>
          <div className="fox-settings-sheet-body">
            <section className="fox-settings-sheet-section">
              <div className="fox-settings-field-row">
                <label className="fox-settings-field"><span>助手名称</span><Input value={assistantDraft.name} onChange={(event) => setAssistantDraft((value) => value ? { ...value, name: event.target.value } : value)} /></label>
                <label className="fox-settings-field"><span>助手介绍</span><Input value={assistantDraft.description} onChange={(event) => setAssistantDraft((value) => value ? { ...value, description: event.target.value } : value)} /></label>
              </div>
              <label className="fox-settings-field"><span>工作要求</span><Textarea className="fox-settings-textarea is-tall" value={assistantDraft.systemPrompt} onChange={(event) => setAssistantDraft((value) => value ? { ...value, systemPrompt: event.target.value } : value)} /></label>
              <label className="fox-settings-field"><span>开场建议（每行一条，最多 6 条）</span><Textarea className="fox-settings-textarea" value={assistantDraft.openingSuggestions} onChange={(event) => setAssistantDraft((value) => value ? { ...value, openingSuggestions: event.target.value } : value)} /></label>
            </section>
          </div>
          <div className="fox-settings-sheet-foot">
            <span className="is-grow" />
            <Button type="button" variant="outline" disabled={general.saving} onClick={() => setAssistantDraft(null)}>取消</Button>
            <Button type="button" disabled={general.saving || !assistantDraft.name.trim() || !assistantDraft.systemPrompt.trim()} onClick={() => void saveAssistant()}>{general.saving && <LoaderCircle className="animate-spin" />}保存</Button>
          </div>
        </>}
      </SheetContent>
    </Sheet>

    <ExpertEditSheet agent={editingExpert} navigate={navigate} onClose={() => setEditingExpertId(null)} onSaved={() => void agents.refresh(false)} />

    <Sheet open={openProject !== null} onOpenChange={(open) => { if (!open) setProjectId(null) }}>
      <SheetContent side="right" className="fox-settings-sheet">
        {openProject && <>
          <SheetHeader className="fox-settings-sheet-head">
            <span className="fox-settings-provider-mark is-large"><FolderOpen /></span>
            <div className="fox-settings-sheet-title"><SheetTitle><span>{openProject.name}</span>{!openProject.pathExists && <Badge variant="destructive" className="fox-settings-pill">路径失效</Badge>}</SheetTitle><SheetDescription title={openProject.rootPath}>{openProject.rootPath}</SheetDescription></div>
          </SheetHeader>
          <div className="fox-settings-sheet-body">
            <section className="fox-settings-sheet-section">
              <div className="fox-settings-sheet-section-head"><h3>执行方式</h3></div>
              <PermissionSelect value={openProject.permissionMode} disabled={busyId === openProject.id} label={`${openProject.name} 执行方式`} className="is-wide" onChange={(value) => void projectsResource.updatePermission(openProject, value)} />
              <p className="fox-settings-sheet-hint">{PERMISSION_LABELS[openProject.permissionMode]}：{openProject.permissionMode === 'read_only' ? 'Agent 只能读取项目文件。' : openProject.permissionMode === 'ask' ? '写入文件或运行命令前先询问。' : '允许在项目内写入文件和运行命令；命令、连接器与项目外写入仍单独审批。'}</p>
            </section>
            <section className="fox-settings-sheet-section">
              <div className="fox-settings-sheet-section-head"><h3>目录</h3></div>
              <dl className="fox-settings-facts">
                <div><dt>版本状态</dt><dd>{openProject.isGitRepository ? openProject.gitBranch || 'Git' : '普通目录'}</dd></div>
                <div><dt>目录占用</dt><dd>{projectStorageLabel(openProject.diskBytes)}</dd></div>
                <div><dt>关联对话</dt><dd>{openProject.conversationCount}</dd></div>
                <div><dt>生成文件</dt><dd>{openProject.artifactCount}</dd></div>
                <div><dt>最近对话</dt><dd title={openProject.recentConversationTitle ?? undefined}>{openProject.recentConversationTitle ?? '暂无'}</dd></div>
                <div><dt>最近生成</dt><dd title={openProject.recentArtifactName ?? undefined}>{openProject.recentArtifactName ?? '暂无'}</dd></div>
              </dl>
            </section>
          </div>
          <div className="fox-settings-sheet-foot">
            <Button type="button" variant="ghost" className="fox-settings-danger" disabled={busyId === openProject.id} onClick={() => { const project = openProject; setProjectId(null); void projectsResource.setArchived(project, true) }}><Archive />归档项目</Button>
            <span className="is-grow" />
            <Button type="button" variant="outline" disabled={!openProject.pathExists} onClick={() => void projectsResource.openRoot(openProject)}><ExternalLink />打开目录</Button>
            <Button type="button" variant="outline" disabled={busyId === openProject.id} onClick={() => void projectsResource.choosePath(openProject)}>{busyId === openProject.id ? <LoaderCircle className="animate-spin" /> : <FolderOpen />}迁移目录</Button>
          </div>
        </>}
      </SheetContent>
    </Sheet>
  </WorkspacePage>
}
