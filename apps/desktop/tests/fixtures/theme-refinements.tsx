import { createRoot } from 'react-dom/client'
import { useState } from 'react'
import { ArrowUp } from 'lucide-react'
import { TooltipProvider } from '../../src/components/ui/tooltip'
import { Button } from '../../src/components/ui/button'
import { InputGroup, InputGroupTextarea } from '../../src/components/ui/input-group'
import '../../src/styles/globals.css'
import '../../src/styles/workbench.css'
import '../../src/styles/workspace-pages.css'

;(globalThis as any).isTauri = true
const { desktopClient } = await import('../../src/features/conversations/api/desktop-client')
const { AgentListPage, AgentDetailPage } = await import('../../src/features/agents/agent-pages')
const { OnboardingPage } = await import('../../src/features/settings/settings-pages')
const { PluginCenterPage } = await import('../../src/features/plugins/plugin-center-page')
const params = new URLSearchParams(location.search)
document.documentElement.dataset.colorTheme = params.get('theme') ?? 'graphite'
document.documentElement.classList.toggle('dark', params.has('dark'))
document.body.style.margin = '0'
desktopClient.getModelService = async () => ({ name: '测试模型', modelId: 'fixture-model' }) as any
desktopClient.getYuxiService = async () => null
const expert = {
  id: 'fixture-expert', name: '资料整理专家', description: '帮助整理文件与核对资料。', agentKind: 'expert', invocationMode: 'inline', visibility: 'expert_center',
  runtimeType: 'pi', defaultModel: 'configured-model', icon: '/mascot/fox_magic.png', category: 'general', openingSuggestions: ['整理项目资料'],
  systemPrompt: '读取资料并按用户指定的目录整理。', isBuiltin: false, packageVersion: '1.0.0', packageSource: 'local', packageId: null, packageHash: 'fixture',
  packageManifest: { skills: [], knowledgeReferences: [], mcpServers: [], allowedTools: [] }, capabilities: [], resources: { tools: [], knowledges: [], mcps: [], skills: [] }, configurableItems: {}, isDefault: false, available: true,
}
desktopClient.listAgents = async () => [expert] as any
desktopClient.syncYuxiAgents = async () => [] as any
for (const name of ['listProjects', 'listKnowledgeBases', 'listSkills', 'listMcpServers', 'listConversations', 'listExpertPackageVersions', 'listDigitalColleagues', 'listDigitalColleagueSchedules', 'listDigitalColleagueChannels', 'listDigitalColleagueTriggers', 'listDigitalColleagueAudit']) (desktopClient as any)[name] = async () => []
const plugin = { id: 'fixture-tool', kind: 'tool', origin: 'builtin', category: '文件', name: '文件读取', description: '读取已授权文件中的文字。', icon: 'file-text', version: '1.0.0', installStatus: 'installed', activation: { mode: 'not_applicable' }, permissions: [], compatible: true }
const pluginGateway: any = { catalogList: async () => ({ items: [plugin], total: 1, categories: ['文件'] }), installationsList: async () => ({ items: [plugin], total: 1 }), operationsList: async () => [], agentScopeList: async () => [] }

function Fixture() {
  const [view, setView] = useState(params.get('surface') ?? 'agents')
  const [focused, setFocused] = useState(false)
  const navigate = (next: string) => setView(next)
  return <TooltipProvider><div className="fox-app fox-shell" style={{ height: '100vh', display: 'block', background: 'var(--background)' }}>
    {view === 'onboarding' ? <OnboardingPage dark={params.has('dark')} navigate={navigate as any} onExit={() => setView('agents')} /> : view === 'plugins' ? <PluginCenterPage gateway={pluginGateway} />
      : view === 'composer' ? <main style={{ maxWidth: 720, margin: '180px auto' }}><p>已有对话输入框样式夹具（不发送请求）</p><div className={`fox-prompt-input ${focused ? 'is-focused' : ''}`}><InputGroup><InputGroupTextarea className="fox-chat-textarea" placeholder="输入消息" onFocus={() => setFocused(true)} onBlur={() => setFocused(false)} /><Button className="fox-send-button" aria-label="发送"><ArrowUp /></Button></InputGroup></div></main>
        : view === 'agent-detail' ? <AgentDetailPage sidebarCollapsed={false} onSidebar={() => {}} navigate={navigate as any} agentId={expert.id} />
          : <AgentListPage sidebarCollapsed={false} onSidebar={() => {}} navigate={navigate as any} />}
  </div></TooltipProvider>
}
;(window as any).__themeFixture = { generation: crypto.randomUUID(), evidence: 'real page components with synthetic desktop APIs; composer is a style probe' }
createRoot(document.getElementById('root')!).render(<Fixture />)
