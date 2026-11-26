import { useEffect, useState, type ReactNode } from 'react'
import { ArrowLeft, ChevronDown, ChevronRight, Plus, Puzzle, Search, ShieldCheck } from 'lucide-react'
import { notify as toast } from '@/features/notifications'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger, runAfterMenuClose } from '@/components/ui/dropdown-menu'
import { FoxEmptyState } from '@/components/fox-ui/empty-state'
import { WorkspacePage } from '@/features/workspace/page-shell'
import type { NavigateWorkspace } from '@/features/workspace/types'
import type { SettingsSectionAnchor } from '@/features/workspace/management-navigation'
import type { PluginCardView, PluginGateway } from '@/features/plugins/model'
import { defaultPluginGateway } from '@/features/plugins/tauri-gateway'
import { matchesPluginQuery, matchesPluginResource, pluginSection, usePluginEntries, type PluginSection } from '@/features/plugins/plugin-browser-data'
import { PluginMark, PluginPolicySheet, PluginStatusBadge, PluginTabs, SkillImportDialog, openPluginDetail } from '@/features/plugins/plugin-controls'
import { useSkills } from './use-skills'
import { useMcpServers } from './use-mcp-servers'
import { clearPluginReturn, readPluginReturn } from './settings-shared'
import { ConnectorSetupSheet, extensionTransportLabel, mcpStatus } from './connector-setup-sheet'
import { SearchServiceSheet } from './search-service-page'

export { extensionTransportLabel, parseEnvironment } from './connector-setup-sheet'

export function ExtensionsSettingsPage({ sidebarCollapsed, onSidebar, navigate, section, initialKind, gateway = defaultPluginGateway }: {
  sidebarCollapsed: boolean
  onSidebar: () => void
  navigate: NavigateWorkspace
  section?: SettingsSectionAnchor
  initialKind?: PluginSection
  gateway?: PluginGateway
}) {
  const startingSection = section === 'search' ? 'plugin' : section === 'skills' ? 'skill' : section === 'mcp' ? 'mcp' : initialKind ?? 'mcp'
  const [kind, setKind] = useState<PluginSection>(startingSection)
  const [search, setSearch] = useState('')
  const [dialog, setDialog] = useState<'mcp' | 'search' | 'skill' | 'policy' | null>(section === 'search' ? 'search' : null)
  const [searchBusy, setSearchBusy] = useState(false)
  const [skillBusy, setSkillBusy] = useState(false)
  const [returnTarget] = useState(readPluginReturn)
  const mcp = useMcpServers()
  const skills = useSkills()
  const entries = usePluginEntries(gateway)
  useEffect(() => { clearPluginReturn() }, [])
  useEffect(() => { if (section === 'search') setDialog('search') }, [section])
  useEffect(() => setKind(startingSection), [startingSection])

  const providers = entries.items.filter(card => pluginSection(card) === 'plugin')
    .sort((left, right) => Number(right.id === 'search:auto') - Number(left.id === 'search:auto'))
  const counts = { mcp: mcp.items.length, plugin: providers.length, skill: skills.items.length }
  const query = search.trim().toLocaleLowerCase()
  const matches = (...texts: string[]) => texts.some(text => text.toLocaleLowerCase().includes(query))
  const closeDialog = () => setDialog(null)
  const updateSearch = async (card: PluginCardView, enabled: boolean) => {
    setSearchBusy(true)
    try { await gateway.setActivation(card.id, { mode: 'global', enabled }); entries.refresh() }
    catch (error) { toast.error('搜索服务未切换', { description: error instanceof Error ? error.message : String(error) }) }
    finally { setSearchBusy(false) }
  }
  const connectorRows = mcp.items.filter(server => matches(server.name, extensionTransportLabel(server.transport), mcpStatus(server).label)).map(server => {
    const card = entries.items.find(item => item.kind === 'mcp' && matchesPluginResource(item.id, 'mcp', server.id))
    const open = () => navigate('settings-plugin-detail', card?.id ?? `mcp:${server.id}`, { documentId: 'mcp' })
    const status = mcpStatus(server)
    return <PluginSettingRow key={server.id} kind="mcp" id={card?.id ?? `mcp:${server.id}`} name={server.name} onOpen={open} badge={<small className="fox-extension-status">{status.label}</small>}
      description={server.lastError || extensionTransportLabel(server.transport)}>
      <Switch checked={server.enabled} disabled={mcp.busyId != null} aria-label={`启用 ${server.name}`}
        onCheckedChange={enabled => void mcp.setEnabled(server.id, enabled).then(entries.refresh)} />
    </PluginSettingRow>
  })
  const skillRows = skills.items.filter(skill => matches(skill.name, skill.description, skill.id)).map(skill => {
    const card = entries.items.find(item => item.kind === 'skill' && matchesPluginResource(item.id, 'skill', skill.id))
    const name = skill.name || skill.id
    return <PluginSettingRow key={skill.id} kind="skill" id={card?.id ?? `skill:${skill.id}`} name={name}
      description={skill.validationError || skill.description}
      badge={card?.activation.mode === 'per_agent' ? <small className="fox-extension-status">{card.activation.enabledAgentCount} 个助手</small> : undefined}
      onOpen={() => navigate('settings-plugin-detail', card?.id ?? `skill:${skill.id}`, { documentId: 'skill' })}>
      <Switch checked={skill.enabled} disabled={!skill.valid || skillBusy} aria-label={`为 Fox 通用助手启用 ${name}`}
        onCheckedChange={async enabled => {
          setSkillBusy(true)
          try { if (await skills.setEnabled(skill.id, enabled)) entries.refresh() }
          finally { setSkillBusy(false) }
        }} />
    </PluginSettingRow>
  })
  const rows = kind === 'mcp' ? connectorRows : kind === 'skill' ? skillRows : providers
    .filter(card => matchesPluginQuery(card, query)).map(card => <PluginSettingRow key={card.id} kind={kind} id={card.id} name={card.name} description={card.description}
      badge={<PluginStatusBadge card={card} />} onOpen={() => openPluginDetail(navigate, card, true)}>
      {card.activation.mode === 'global' && <Switch checked={card.activation.enabled} disabled={searchBusy || entries.loading || (card.id === 'search:auto' && card.activation.enabled)}
        aria-label={`使用 ${card.name} 搜索`} onCheckedChange={enabled => void updateSearch(card, enabled)} />}
    </PluginSettingRow>)
  const loading = kind === 'mcp' ? mcp.loading : kind === 'skill' ? skills.loading : entries.loading
  const error = kind === 'mcp' ? mcp.error : kind === 'skill' ? skills.error : entries.error
  const refresh = kind === 'mcp' ? mcp.refresh : kind === 'skill' ? skills.refresh : entries.refresh

  return <WorkspacePage title="插件" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
    <div className="fox-settings-page fox-extension-settings">
      <header className="fox-extension-page-head"><h1>插件</h1>
        <div>
          {returnTarget && <Button variant="ghost" onClick={() => navigate(returnTarget.view, returnTarget.entityId, returnTarget.documentId ? { documentId: returnTarget.documentId } : undefined)}><ArrowLeft />返回 {returnTarget.label}</Button>}
          <Button variant="outline" onClick={() => navigate('plugins', kind)}>浏览目录</Button>
          <DropdownMenu><DropdownMenuTrigger asChild><Button><Plus />添加<ChevronDown /></Button></DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={() => runAfterMenuClose(() => setDialog('mcp'))}><Plus />添加连接器</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => runAfterMenuClose(() => setDialog('skill'))}><Puzzle />导入技能</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => runAfterMenuClose(() => setDialog('policy'))}><ShieldCheck />插件策略与 Hook</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </header>
      <div className="fox-extension-toolbar">
        <PluginTabs section={kind} counts={counts} onChange={next => {
          setKind(next)
          navigate('settings-extensions', next)
        }} />
        <label className="fox-extension-search"><Search size={14} /><Input placeholder="搜索" aria-label="搜索已安装插件" value={search} onChange={event => setSearch(event.target.value)} /></label>
      </div>
      {kind === 'skill' && <div className="fox-extension-settings-action"><Button variant="ghost" size="sm" disabled={skills.loading} onClick={() => { void skills.refresh(); entries.refresh() }}>重新扫描</Button></div>}
      {error && <div className="fox-settings-error" role="alert">{error}<Button size="sm" variant="ghost" onClick={() => void refresh()}>重试</Button></div>}
      {rows.length > 0 ? <div className="fox-extension-list">{rows}</div>
        : loading ? <p className="fox-settings-empty" role="status">正在加载…</p>
          : !error && <FoxEmptyState title={query ? '没有匹配的条目' : '当前分类没有已安装条目'} />}
    </div>
    {dialog === 'mcp' && <ConnectorSetupSheet mcp={mcp} onClose={closeDialog} onSaved={entries.refresh} />}
    <SearchServiceSheet open={dialog === 'search'} onOpenChange={open => { if (!open) closeDialog() }} onSaved={entries.refresh} />
    <SkillImportDialog open={dialog === 'skill'} onOpenChange={open => { if (!open) closeDialog() }} onRescan={() => {
      setKind('skill'); navigate('settings-extensions', 'skill'); void skills.refresh(); entries.refresh()
    }} />
    <PluginPolicySheet open={dialog === 'policy'} onOpenChange={open => { if (!open) closeDialog() }} navigate={navigate} />
  </WorkspacePage>
}

function PluginSettingRow({ kind, id, name, description, badge, onOpen, children }: {
  kind: PluginSection; id: string; name: string; description: string; badge?: ReactNode; onOpen: () => void; children?: ReactNode
}) {
  return <div className="fox-extension-list-row">
    <button className="fox-extension-list-main" type="button" onClick={onOpen} aria-label={`查看 ${name} 详情`}>
      <PluginMark kind={kind} id={id} />
      <span className="fox-extension-list-copy"><span><strong>{name}</strong>{badge}</span><small>{description}</small></span>
    </button>
    {children}
    <Button variant="ghost" size="icon-sm" aria-label={`打开 ${name}`} onClick={onOpen}><ChevronRight /></Button>
  </div>
}
