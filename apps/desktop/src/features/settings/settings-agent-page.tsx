import { useEffect, useState } from 'react'
import { Bot, ChevronRight, Info, LoaderCircle, Plus } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { WorkspacePage } from '@/features/workspace/page-shell'
import type { NavigateWorkspace } from '@/features/workspace/types'
import type { SettingsSectionAnchor } from '@/features/workspace/management-navigation'
import { desktopClient, desktopErrorDetails, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { AgentRecord } from '@/features/conversations/model/types'
import { useAgents } from '@/features/agents/use-agents'
import { asAgentCard } from '@/features/agents/agent-card-data'
import { AgentEditorProfileFields, AgentEditorResourcesFields, useAgentEditor } from '@/features/agents/agent-pages'
import { notify as toast } from '@/features/notifications'
import { AgentBackendSettingsSection } from './agent-backend-settings'
import { AgentServiceSettings } from './model-agent/agent-service-settings'
import { SettingsGroup, SettingsPageLayout, useScrollToSettingsSection } from './settings-layout'
import { rememberPluginReturn } from './settings-shared'

function expertSourceLabel(agent: AgentRecord) {
  if (agent.packageSource === 'imported') return `专家包 v${agent.packageVersion}`
  if (agent.packageSource === 'builtin') return '内置专家'
  if (agent.runtimeType === 'pi') return '本机专家'
  return '资料库服务专家'
}

type GeneralAssistantDraft = { name: string; description: string; systemPrompt: string; openingSuggestions: string }

function useGeneralAssistant() {
  const [assistant, setAssistant] = useState<AgentRecord | null>(null)
  const [loading, setLoading] = useState(desktopRuntimeAvailable)
  const [saving, setSaving] = useState(false)
  useEffect(() => {
    let cancelled = false
    if (!desktopRuntimeAvailable) { setLoading(false); return }
    void desktopClient.listAgents()
      .then(items => { if (!cancelled) setAssistant(items.find(item => item.id === 'fox-general') ?? null) })
      .catch(cause => { if (!cancelled) toast.error('通用助手配置读取失败', { description: desktopErrorDetails(cause).message }) })
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
        openingSuggestions: draft.openingSuggestions.split(/\r?\n/).map(item => item.trim()).filter(Boolean).slice(0, 6),
        packageManifest: assistant.packageManifest,
      })
      setAssistant(updated)
      toast.success('Fox 通用助手设置已保存')
      return true
    } catch (cause) {
      toast.error('保存通用助手设置失败', { description: desktopErrorDetails(cause).message })
      return false
    } finally { setSaving(false) }
  }
  return { assistant, loading, saving, save }
}

/** Keeps existing expert deep links and plugin return flows working. */
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
  return <Sheet open={Boolean(agent)} onOpenChange={open => { if (!open) onClose() }}>
    <SheetContent side="right" className="fox-settings-sheet fox-settings-expert-sheet">
      {agent && card && <>
        <SheetHeader className="fox-settings-sheet-head">
          <span className="fox-settings-provider-mark is-large is-avatar"><img src={card.image} alt="" /></span>
          <div className="fox-settings-sheet-title"><SheetTitle><span>{editor.draft.name || agent.name}</span>{agent.isDefault && <Badge variant="secondary" className="fox-settings-pill">默认</Badge>}</SheetTitle><SheetDescription>{expertSourceLabel(agent)}</SheetDescription></div>
        </SheetHeader>
        <div className="fox-settings-sheet-body">
          <Tabs className="fox-settings-segmented fox-settings-sheet-tabs" value={tab} onValueChange={value => { if (value === 'profile' || value === 'resources') setTab(value) }}>
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
  const general = useGeneralAssistant()
  const agents = useAgents()
  const [assistantDraft, setAssistantDraft] = useState<GeneralAssistantDraft | null>(null)
  const [editingExpertId, setEditingExpertId] = useState<string | null>(agentId ?? null)
  useEffect(() => { if (agentId) setEditingExpertId(agentId) }, [agentId])
  const editingExpert = editingExpertId ? agents.agents.find(item => item.id === editingExpertId) ?? null : null
  const openAssistant = () => {
    const assistant = general.assistant
    if (!assistant) return
    setAssistantDraft({ name: assistant.name, description: assistant.description, systemPrompt: assistant.systemPrompt, openingSuggestions: assistant.openingSuggestions.join('\n') })
  }

  return <WorkspacePage title="Agent" subtitle="新对话与运行引擎" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
    <SettingsPageLayout title="Agent" description="选择新对话使用的运行引擎，配置 Fox 通用助手和外部 Agent 的工作方式。">
      {desktopRuntimeAvailable && <AgentBackendSettingsSection client={desktopClient} onOpenModelSettings={() => navigate('settings-models')} />}
      <SettingsGroup title="Fox 通用助手">
        <div className="fox-settings-providers"><div className="fox-settings-provider">
          <button type="button" className="fox-settings-provider-main" disabled={!general.assistant} onClick={openAssistant} aria-haspopup="dialog" aria-label="编辑 Fox 通用助手">
            <span className="fox-settings-provider-mark is-avatar">{general.assistant?.icon ? <img src={general.assistant.icon} alt="" /> : general.loading ? <LoaderCircle className="animate-spin" /> : <Bot />}</span>
            <span className="fox-settings-provider-text"><span className="fox-settings-provider-name"><b>{general.assistant?.name ?? (general.loading ? '正在读取通用助手…' : '暂时无法读取通用助手')}</b></span><small>{general.assistant?.description || '名称、简介、工作要求和新对话开场建议'}</small></span>
            <ChevronRight className="fox-settings-provider-chevron" />
          </button>
        </div></div>
      </SettingsGroup>
      {desktopRuntimeAvailable && <AgentServiceSettings onOpenModelSettings={() => navigate('settings-models')} />}
      <p className="fox-settings-note"><Info />专家的内容、技能和资料库在专家中心管理。项目权限位于「权限与安全」，新伙伴默认值位于「伙伴」。</p>
      <Button variant="ghost" className="fox-settings-page-link" onClick={() => navigate('agents')}>打开专家中心<ChevronRight /></Button>
    </SettingsPageLayout>

    <Sheet open={assistantDraft !== null} onOpenChange={open => { if (!open) setAssistantDraft(null) }}>
      <SheetContent side="right" className="fox-settings-sheet">
        {assistantDraft && <>
          <SheetHeader className="fox-settings-sheet-head">
            <span className="fox-settings-provider-mark is-large is-avatar">{general.assistant?.icon ? <img src={general.assistant.icon} alt="" /> : <Bot />}</span>
            <div className="fox-settings-sheet-title"><SheetTitle><span>Fox 通用助手</span></SheetTitle><SheetDescription>应用于之后新建的通用助手对话，不改变专家和远程助手。</SheetDescription></div>
          </SheetHeader>
          <div className="fox-settings-sheet-body"><section className="fox-settings-sheet-section">
            <div className="fox-settings-field-row">
              <label className="fox-settings-field"><span>助手名称</span><Input value={assistantDraft.name} onChange={event => setAssistantDraft(value => value ? { ...value, name: event.target.value } : value)} /></label>
              <label className="fox-settings-field"><span>助手介绍</span><Input value={assistantDraft.description} onChange={event => setAssistantDraft(value => value ? { ...value, description: event.target.value } : value)} /></label>
            </div>
            <label className="fox-settings-field"><span>工作要求</span><Textarea className="fox-settings-textarea is-tall" value={assistantDraft.systemPrompt} onChange={event => setAssistantDraft(value => value ? { ...value, systemPrompt: event.target.value } : value)} /></label>
            <label className="fox-settings-field"><span>开场建议（每行一条，最多 6 条）</span><Textarea className="fox-settings-textarea" value={assistantDraft.openingSuggestions} onChange={event => setAssistantDraft(value => value ? { ...value, openingSuggestions: event.target.value } : value)} /></label>
          </section></div>
          <div className="fox-settings-sheet-foot"><span className="is-grow" /><Button type="button" variant="outline" disabled={general.saving} onClick={() => setAssistantDraft(null)}>取消</Button><Button type="button" disabled={general.saving || !assistantDraft.name.trim() || !assistantDraft.systemPrompt.trim()} onClick={() => { if (assistantDraft) void general.save(assistantDraft).then(saved => { if (saved) setAssistantDraft(null) }) }}>{general.saving && <LoaderCircle className="animate-spin" />}保存</Button></div>
        </>}
      </SheetContent>
    </Sheet>
    <ExpertEditSheet agent={editingExpert} navigate={navigate} onClose={() => setEditingExpertId(null)} onSaved={() => void agents.refresh(false)} />
  </WorkspacePage>
}
