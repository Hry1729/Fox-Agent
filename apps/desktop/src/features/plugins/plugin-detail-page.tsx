import { useEffect, useState } from 'react'
import { ChevronRight, Copy, ExternalLink, Info, MoreHorizontal, Settings2, Sparkles, Trash2, Zap } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, runAfterMenuClose } from '@/components/ui/dropdown-menu'
import { confirmDialog } from '@/components/ui/confirm-dialog'
import { FoxEmptyState } from '@/components/fox-ui/empty-state'
import { SkeletonContent } from '@/components/ui/skeleton'
import { notify as toast } from '@/features/notifications'
import { desktopClient, desktopErrorDetails, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { NavigateWorkspace } from '@/features/workspace/types'
import { DetailPromptCards } from '@/features/workspace/detail-prompt-cards'
import { ConnectorSetupSheet, BUILTIN_CONNECTOR_ID, extensionTransportLabel } from '@/features/settings/connector-setup-sheet'
import { SearchServiceSheet, searchProviderForPlugin } from '@/features/settings/search-service-page'
import { useMcpServers } from '@/features/settings/use-mcp-servers'
import { useSkills } from '@/features/settings/use-skills'
import { formatDateTime } from '@/lib/time-format'
import type { PluginAgentScopeDTO, PluginCardView, PluginGateway } from './model'
import { matchesPluginResource, pluginSectionLabels, pluginStatus, usePluginEntries, type PluginSection } from './plugin-browser-data'
import { PluginMark, PluginStatusBadge } from './plugin-controls'
import { defaultPluginGateway } from './tauri-gateway'
import { CatalogConnectSheet } from './catalog-connect-sheet'
import { findConnectorPreset } from './connector-presets'
import { pluginExamplePrompts } from './plugin-examples'

export default function PluginDetailPage({ id, kind, fromSettings, navigate, gateway = defaultPluginGateway }: {
  id: string; kind: PluginSection; fromSettings: boolean; navigate: NavigateWorkspace; gateway?: PluginGateway
}) {
  const entries = usePluginEntries(gateway, fromSettings ? undefined : kind)
  const card = entries.items.find(item => item.id === id)
  const back = () => navigate(fromSettings ? 'settings-extensions' : 'plugins', kind)
  return <article className={`fox-plugin-center fox-extension-detail ${fromSettings ? 'is-settings' : ''}`}>
    <nav className="fox-extension-breadcrumb" aria-label="插件路径">
      <button type="button" onClick={() => navigate(fromSettings ? 'settings-extensions' : 'plugins')}>{fromSettings ? '设置 · 插件' : '插件'}</button><ChevronRight />
      <button type="button" onClick={() => fromSettings || !card ? back() : navigate('plugins', kind, { documentId: card.category })}>
        {card?.category || pluginSectionLabels[kind]}
      </button>
      {card && <><ChevronRight /><span aria-current="page">{card.name}</span></>}
    </nav>
    {entries.loading && !card ? <SkeletonContent label="正在载入插件详情" lines={4} avatar />
      : card ? <PluginDetails key={card.id} card={card} kind={kind} gateway={gateway} navigate={navigate} refresh={entries.refresh} back={back} />
        : <FoxEmptyState variant={entries.error ? 'error' : 'empty'} title={entries.error ? '无法加载插件' : '条目已移除或不可用'} description={entries.error ?? undefined}
          action={<Button variant="outline" onClick={entries.error ? entries.refresh : back}>{entries.error ? '重试' : '返回列表'}</Button>} />}
  </article>
}

function PluginDetails({ card, kind, gateway, navigate, refresh, back }: {
  card: PluginCardView; kind: PluginSection; gateway: PluginGateway; navigate: NavigateWorkspace; refresh: () => void; back: () => void
}) {
  const mcp = useMcpServers(kind === 'mcp')
  const preset = kind === 'mcp' ? findConnectorPreset(card.id) : undefined
  const skills = useSkills('fox-general', kind === 'skill')
  const server = kind === 'mcp' ? mcp.items.find(item => matchesPluginResource(card.id, 'mcp', item.id)) : undefined
  const skill = kind === 'skill' ? skills.items.find(item => matchesPluginResource(card.id, 'skill', item.id)) : undefined
  const [manage, setManage] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const configured = card.installStatus !== 'not_installed'
  const provider = kind === 'plugin' ? searchProviderForPlugin(card.id) : undefined
  const enabled = card.activation.mode === 'global' && card.activation.enabled
  const hasTools = server?.status === 'connected' && (server.toolCount ?? 0) > 0
  const canTry = configured && pluginStatus(card) !== 'error'
    && (kind !== 'mcp' || hasTools)
    && (card.activation.mode === 'not_applicable' || enabled || skill?.enabled === true)
  const prompts = pluginExamplePrompts(card, kind)
  const run = (value: string) => navigate('chat', undefined, { draftPrompt: value })
  const activate = async () => {
    setBusy(true)
    setError('')
    try { await gateway.setActivation(card.id, { mode: 'global', enabled: !enabled }); refresh() }
    catch (cause) { setError(desktopErrorDetails(cause).message) }
    finally { setBusy(false) }
  }
  const copyInfo = async () => {
    try {
      await navigator.clipboard.writeText([card.name, card.description, card.id].filter(Boolean).join('\n'))
      toast.success('已复制插件信息')
    } catch { toast.error('无法复制插件信息') }
  }
  const remove = async () => {
    if (!server || !await confirmDialog({ title: '删除连接器', description: `删除「${server.name}」及其已保存的凭证？`, confirmLabel: '删除', destructive: true })) return
    if (await mcp.remove(server.id)) { toast.success('连接器已删除'); back() }
  }
  const connection = server?.status === 'connected' ? mcp.tests[server.id] : undefined
  const publisher = preset?.id === 'gmail' ? 'Google'
    : preset?.id === 'fliggy' ? '飞猪'
    : preset ? '连接器目录'
    : card.origin === 'builtin' ? 'Fox' : card.origin === 'local' ? '本机配置' : card.origin === 'official' ? '官方目录' : '社区目录'
  const canToggle = card.activation.mode === 'global' && configured && !(provider === 'auto' && enabled)
  const notice = error || mcp.error || skills.error || card.incompatibilityReason || card.lastError
  return <>
    <header className="fox-extension-hero">
      <PluginMark kind={kind} name={card.icon} id={card.id} />
      <div className="fox-extension-hero-copy">
        <div><h1>{card.name}</h1><PluginStatusBadge card={card} label={kind === 'mcp' && canTry ? '已启用' : undefined} /></div>
        <p>{preset?.id === 'gmail' || preset?.id === 'fliggy' ? `由 ${publisher} 发布` : publisher}{card.description && ` · ${card.description}`}</p>
      </div>
      <div className="fox-extension-hero-actions">
        <DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" aria-label="更多插件操作"><MoreHorizontal /></Button></DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {preset && (desktopRuntimeAvailable
              ? <DropdownMenuItem onSelect={() => {
                void desktopClient.openExternalUrl(preset.source).catch(cause => toast.error('无法打开接入文档', { description: desktopErrorDetails(cause).message }))
              }}><ExternalLink />接入文档</DropdownMenuItem>
              : <DropdownMenuItem asChild><a href={preset.source} target="_blank" rel="noreferrer"><ExternalLink />接入文档</a></DropdownMenuItem>)}
            <DropdownMenuItem onSelect={() => void copyInfo()}><Copy />复制信息</DropdownMenuItem>
            {server && <DropdownMenuItem disabled={!server.enabled || mcp.busyId !== null} onSelect={() => {
              void mcp.test(server.id).then(result => { if (result) toast.success(`已发现 ${result.toolCount} 个工具`); refresh() })
            }}><Zap />测试连接</DropdownMenuItem>}
            {kind === 'mcp' && canTry && canToggle && <DropdownMenuItem disabled={busy} onSelect={() => void activate()}><Settings2 />停用连接器</DropdownMenuItem>}
            {server && server.id !== BUILTIN_CONNECTOR_ID && <><DropdownMenuSeparator /><DropdownMenuItem variant="destructive" disabled={mcp.busyId != null}
              onSelect={() => runAfterMenuClose(() => void remove())}><Trash2 />移除配置</DropdownMenuItem></>}
          </DropdownMenuContent>
        </DropdownMenu>
        {kind !== 'skill' && (kind !== 'mcp' || server || !configured) && <Button variant={kind === 'mcp' && !configured ? 'default' : 'outline'} onClick={() => setManage(true)}>
          <Settings2 />{preset?.setup.pending ? '接入条件' : kind === 'mcp' && !server ? '配置接入' : '管理'}
        </Button>}
        {canToggle && !(kind === 'mcp' && canTry) && <Button variant={canTry ? 'outline' : 'default'} disabled={busy || !card.compatible} onClick={() => void activate()}>
          {enabled ? (kind === 'plugin' ? '恢复免 Key 搜索' : '停用') : (kind === 'plugin' ? '选用' : '启用')}
        </Button>}
        {canTry && <Button onClick={() => run(prompts[0])}><Sparkles />立即试用</Button>}
      </div>
    </header>
    <DetailPromptCards label="示例指令" name={card.name} mark={<PluginMark kind={kind} name={card.icon} id={card.id} />}
      prompts={prompts} onSelect={run} />
    <section className="fox-extension-detail-section">
      <h2>概述</h2>
      {card.description && <p>{card.description}</p>}
      {notice && <div className="fox-extension-notice" role="alert">{notice}</div>}
      {kind === 'mcp' && !canTry && <div className="fox-extension-notice"><Info size={16} aria-hidden /><span>{preset?.setup.pending ?? (!configured ? preset?.setup.guide : null) ?? '启用连接器并测试连接后，可以查看工具清单和试用。'}</span></div>}
      {provider && <div className="fox-extension-notice">Fox 每次只使用一种搜索服务。停用当前服务会切回免 Key 搜索。</div>}
    </section>
    {kind === 'skill' ? <>
      {Boolean(card.permissions.length) && <section className="fox-extension-detail-section"><h2>依赖工具</h2>
        <ul className="fox-extension-tool-list">{card.permissions.map(permission => <li key={permission}>{permission}</li>)}</ul>
      </section>}
      {configured && <PluginSkillScope card={card} gateway={gateway} onChange={refresh} />}
      {skill?.instructions && <section className="fox-extension-detail-section"><details><summary>技能指令</summary><pre className="fox-extension-instructions">{skill.instructions}</pre></details></section>}
    </> : <section className="fox-extension-detail-section">
      <h2>应用与工具</h2>
      <div className="fox-extension-application"><PluginMark kind={kind} name={card.icon} id={card.id} /><div><strong>{card.name}</strong>
        {server && <small>{extensionTransportLabel(server.transport)}</small>}</div></div>
      {kind === 'mcp' && (connection ? <ul className="fox-extension-tool-list">{connection.tools.map(tool => <li key={tool.name}><b>{tool.name}</b>{tool.description && <small>{tool.description}</small>}</li>)}</ul>
        : server && <p>{server.toolCount != null ? `上次检查发现 ${server.toolCount} 个工具。测试连接可查看工具清单。` : '测试连接后可查看工具清单。'}</p>)}
      {kind === 'plugin' && <ul className="fox-extension-tool-list"><li>网页搜索</li></ul>}
    </section>}
    <section className="fox-extension-detail-section"><h2>信息</h2>
      <dl className="fox-extension-facts">
        <div><dt>标识</dt><dd>{card.id}</dd></div>
        <div><dt>来源</dt><dd>{publisher}</dd></div>
        <div><dt>分类</dt><dd>{card.category}</dd></div>
        {card.version && <div><dt>版本</dt><dd>{card.version}</dd></div>}
        {card.updatedAt && <div><dt>更新时间</dt><dd>{formatDateTime(card.updatedAt)}</dd></div>}
        {skill && <div><dt>来源文件</dt><dd>{skill.sourcePath}</dd></div>}
        {server && <div><dt>连接类型</dt><dd>{extensionTransportLabel(server.transport)}</dd></div>}
      </dl>
    </section>
    {kind === 'mcp' && manage && (preset ? <CatalogConnectSheet item={preset} mcp={mcp} onChange={refresh} onClose={() => setManage(false)} /> : <ConnectorSetupSheet mcp={mcp} server={server} suggestedName={card.name} onClose={() => setManage(false)} onSaved={() => { refresh(); if (!server) back() }} />)}
    {provider && <SearchServiceSheet open={manage} initialProvider={provider} onOpenChange={setManage} onSaved={refresh} />}
  </>
}

function PluginSkillScope({ card, gateway, onChange }: { card: PluginCardView; gateway: PluginGateway; onChange: () => void }) {
  const [agents, setAgents] = useState<PluginAgentScopeDTO[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  useEffect(() => {
    let active = true
    void gateway.agentScopeList(card.id).then(items => { if (active) setAgents(items) })
      .catch((cause: unknown) => { if (active) setError(cause instanceof Error ? cause.message : String(cause)) })
      .finally(() => { if (active) setLoading(false) })
    return () => { active = false }
  }, [card.id, gateway])
  return <section className="fox-extension-detail-section"><h2>适用助手范围</h2>
    {loading && <p role="status">正在读取助手设置…</p>}
    {error && <div className="fox-extension-notice" role="alert">{error}</div>}
    {!loading && !agents.length && !error && <p>当前没有可配置的本机助手。</p>}
    {agents.length > 0 && <div className="fox-extension-list">{agents.map(agent => <label className="fox-extension-scope-row" key={agent.agentId}>
      <span>{agent.agentName}</span><Switch checked={agent.enabled} disabled={busy || !card.compatible} aria-label={`为 ${agent.agentName} 启用 ${card.name}`}
        onCheckedChange={async enabled => {
          setBusy(true); setError('')
          try {
            await gateway.setActivation(card.id, { mode: 'per_agent', agentId: agent.agentId, enabled })
            setAgents(items => items.map(item => item.agentId === agent.agentId ? { ...item, enabled } : item))
            onChange()
          } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
          finally { setBusy(false) }
        }} />
    </label>)}</div>}
  </section>
}
