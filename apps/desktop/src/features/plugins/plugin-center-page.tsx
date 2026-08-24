import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react'
import {
  ArrowLeft,
  Cable,
  Check,
  CircleAlert,
  CircleCheck,
  Database,
  Download,
  File,
  FileText,
  Folder,
  Globe2,
  HardDrive,
  LoaderCircle,
  NotebookPen,
  PackageCheck,
  Plus,
  Puzzle,
  RefreshCw,
  Search,
  Settings2,
  Sparkles,
  Terminal,
  Wrench,
  X,
  type LucideIcon,
} from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import '@/styles/plugin-center.css'
import { defaultPluginGateway } from './tauri-gateway'
import type {
  PluginAgentScopeDTO,
  PluginCardView,
  PluginGateway,
  PluginKind,
  PluginOperationDTO,
} from './model'
import {
  countInstalledPlugins,
  filterPluginCards,
  pluginInstallAction,
  pluginInstallStatusLabel,
  pluginKindLabel,
  sortPluginCards,
} from './selectors'

const iconMap: Record<string, LucideIcon> = {
  cable: Cable,
  database: Database,
  file: File,
  'file-text': FileText,
  folder: Folder,
  globe: Globe2,
  'hard-drive': HardDrive,
  notebook: NotebookPen,
  puzzle: Puzzle,
  sparkles: Sparkles,
  terminal: Terminal,
  wrench: Wrench,
}

const kindIcons: Record<PluginKind, LucideIcon> = {
  mcp: Cable,
  skill: Sparkles,
  tool: Wrench,
}

const kindOrder: PluginKind[] = ['mcp', 'skill', 'tool']

export interface PluginCenterPageProps {
  gateway?: PluginGateway
  onAddPlugin?: () => void
  onConfigureAgent?: (plugin: PluginCardView) => void
  className?: string
}

function PluginIcon({ name }: { name?: string }) {
  const Icon = iconMap[name ?? 'puzzle'] ?? Puzzle
  return <Icon aria-hidden="true" />
}

function KindIcon({ kind }: { kind: PluginKind }) {
  const Icon = kindIcons[kind]
  return <Icon aria-hidden="true" />
}

function StatusBadge({ card }: { card: PluginCardView }) {
  if (card.installStatus === 'installed') {
    return <Badge variant="outline" className="fox-plugin-status fox-plugin-status-installed"><CircleCheck />已安装</Badge>
  }
  if (card.installStatus === 'installing' || card.installStatus === 'uninstalling') {
    return <Badge variant="outline" className="fox-plugin-status fox-plugin-status-progress"><LoaderCircle className="animate-spin" />{pluginInstallStatusLabel(card.installStatus)}</Badge>
  }
  if (card.installStatus === 'install_failed') {
    return <Badge variant="outline" className="fox-plugin-status fox-plugin-status-error"><CircleAlert />安装失败</Badge>
  }
  if (card.installStatus === 'update_available') {
    return <Badge variant="outline" className="fox-plugin-status fox-plugin-status-update"><Download />可更新</Badge>
  }
  return <Badge variant="outline" className="fox-plugin-status">未安装</Badge>
}

function PluginCard({ card, onInstallAction }: { card: PluginCardView; onInstallAction: (card: PluginCardView) => void }) {
  const action = pluginInstallAction(card)
  const canInstall = action.action === 'install' || action.action === 'update'
  return (
    <Card className="fox-plugin-card" data-plugin-id={card.id} data-install-status={card.installStatus}>
      <div className="fox-plugin-card-main">
        <div className="fox-plugin-card-heading">
          <span className="fox-plugin-icon"><PluginIcon name={card.icon} /></span>
          <span className="fox-plugin-card-title"><strong>{card.name}</strong><small>{card.category}</small></span>
          {canInstall ? <Button className="fox-plugin-card-install" type="button" variant="outline" size="sm" disabled={action.disabled} onClick={() => onInstallAction(card)}><Download />{action.label}</Button> : <StatusBadge card={card} />}
        </div>
        <p>{card.description}</p>
      </div>
    </Card>
  )
}

function InstalledPluginCard({ card, busy, onToggle }: { card: PluginCardView; busy: boolean; onToggle: (enabled: boolean) => void }) {
  const enabled = card.activation.mode === 'global'
    ? card.activation.enabled
    : card.activation.mode === 'per_agent'
      ? card.activation.enabledAgentCount > 0
      : true
  const configurable = card.activation.mode !== 'not_applicable'
  return (
    <Card className="fox-plugin-installed-card">
      <span className="fox-plugin-installed-card-icon"><PluginIcon name={card.icon} /></span>
      <span className="fox-plugin-installed-card-copy"><span><strong>{card.name}</strong><Badge variant="secondary">{pluginKindLabel(card.kind)}</Badge></span><small>{card.description}</small></span>
      <Switch checked={enabled} disabled={!configurable || busy} onCheckedChange={onToggle} aria-label={`${enabled ? '停用' : '启用'}${card.name}`} size="sm" />
    </Card>
  )
}

function OperationRow({ operation }: { operation: PluginOperationDTO }) {
  const isDone = operation.status === 'completed'
  const isFailed = operation.status === 'failed' || operation.status === 'cancelled'
  return (
    <div className="fox-plugin-operation" data-operation-id={operation.id}>
      <span className={`fox-plugin-operation-icon ${isDone ? 'is-done' : isFailed ? 'is-failed' : ''}`}>
        {isDone ? <Check /> : isFailed ? <CircleAlert /> : <LoaderCircle className="animate-spin" />}
      </span>
      <span className="fox-plugin-operation-copy"><strong>{operation.operation === 'install' ? '安装' : operation.operation === 'uninstall' ? '卸载' : operation.operation === 'update' ? '更新' : '导入'}插件</strong><small>{operation.message ?? operation.stage ?? '处理中'}</small></span>
      <span className="fox-plugin-operation-progress"><span><i style={{ width: `${operation.progress}%` }} /></span><small>{operation.progress}%</small></span>
    </div>
  )
}

function ScopeDialog({ plugin, scope, loading, onClose, onToggle }: { plugin: PluginCardView; scope: PluginAgentScopeDTO[]; loading: boolean; onClose: () => void; onToggle: (agent: PluginAgentScopeDTO) => void }) {
  return (
    <div className="fox-plugin-scope-backdrop" role="presentation" onMouseDown={(event) => { if (event.currentTarget === event.target) onClose() }}>
      <section className="fox-plugin-scope-dialog" role="dialog" aria-modal="true" aria-labelledby="fox-plugin-scope-title">
        <header><span><Settings2 /></span><div><h2 id="fox-plugin-scope-title">配置启用范围</h2><p>{plugin.name} 只对选定的 Agent 生效。</p></div><Button type="button" variant="ghost" size="icon" aria-label="关闭配置范围" onClick={onClose}><X /></Button></header>
        {loading ? <div className="fox-plugin-scope-loading"><LoaderCircle className="animate-spin" />读取 Agent 配置中…</div> : <div className="fox-plugin-scope-list">{scope.map((agent) => <label key={agent.agentId}><span><input type="checkbox" checked={agent.enabled} onChange={() => onToggle(agent)} /><b>{agent.agentName}</b></span><small>{agent.enabled ? '已启用' : '未启用'}</small></label>)}</div>}
        <footer><Button type="button" variant="outline" size="sm" onClick={onClose}>完成</Button></footer>
      </section>
    </div>
  )
}

function EmptyState({ children }: { children: ReactNode }) {
  return <div className="fox-plugin-empty"><Puzzle /><span>{children}</span></div>
}

export function PluginCenterPage({ gateway: providedGateway, onAddPlugin, onConfigureAgent, className }: PluginCenterPageProps) {
  const gateway = providedGateway ?? defaultPluginGateway
  const [kind, setKind] = useState<PluginKind>('mcp')
  const [search, setSearch] = useState('')
  const [installedSearch, setInstalledSearch] = useState('')
  const [category, setCategory] = useState<string | undefined>()
  const [catalog, setCatalog] = useState<PluginCardView[]>([])
  const [installed, setInstalled] = useState<PluginCardView[]>([])
  const [operations, setOperations] = useState<PluginOperationDTO[]>([])
  const [categories, setCategories] = useState<string[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [showInstalled, setShowInstalled] = useState(false)
  const [activationBusy, setActivationBusy] = useState<Set<string>>(new Set())
  const [scopePlugin, setScopePlugin] = useState<PluginCardView | null>(null)
  const [scope, setScope] = useState<PluginAgentScopeDTO[]>([])
  const [scopeLoading, setScopeLoading] = useState(false)

  const reload = useCallback(async () => {
    setLoading(true)
    try {
      const [catalogResult, installedResult, operationResult] = await Promise.all([
        gateway.catalogList({ kind, search, category, pageSize: 50 }),
        gateway.installationsList(),
        gateway.operationsList(),
      ])
      setCatalog(catalogResult.items)
      setCategories(catalogResult.categories)
      setInstalled(installedResult.items)
      setOperations(operationResult)
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : '插件中心暂时无法加载。')
    } finally {
      setLoading(false)
    }
  }, [category, gateway, kind, search])

  useEffect(() => { void reload() }, [reload])

  const visibleCards = useMemo(() => sortPluginCards(filterPluginCards(catalog, { kind, search, category })), [catalog, category, kind, search])
  const featuredCards = visibleCards.slice(0, 3)
  const installedCards = useMemo(() => {
    const normalizedSearch = installedSearch.trim().toLocaleLowerCase()
    return sortPluginCards(installed.filter((card) => !normalizedSearch || [card.name, card.description, card.category, pluginKindLabel(card.kind)].some((value) => value.toLocaleLowerCase().includes(normalizedSearch))))
  }, [installed, installedSearch])
  const installedCount = countInstalledPlugins(installed)

  const updateCard = (next: PluginCardView) => {
    setCatalog((current) => current.map((item) => item.id === next.id ? next : item))
    setInstalled((current) => current.map((item) => item.id === next.id ? next : item))
  }

  const handleInstallAction = async (card: PluginCardView) => {
    const action = pluginInstallAction(card).action
    if (action !== 'install' && action !== 'update') return
    const operation = action === 'install' ? await gateway.install(card.id) : await gateway.update(card.id)
    setOperations((current) => [operation, ...current.filter((item) => item.id !== operation.id)])
    await reload()
  }

  const handleToggle = async (card: PluginCardView, enabled: boolean) => {
    if (card.activation.mode !== 'global') return
    const next = await gateway.setActivation(card.id, { mode: 'global', enabled })
    updateCard(next)
  }

  const handleConfigure = async (card: PluginCardView) => {
    onConfigureAgent?.(card)
    setScopePlugin(card)
    setScope([])
    setScopeLoading(true)
    try {
      setScope(await gateway.agentScopeList(card.id))
    } catch {
      setScope([])
    } finally {
      setScopeLoading(false)
    }
  }

  const handleScopeToggle = async (agent: PluginAgentScopeDTO) => {
    if (!scopePlugin) return
    const next = await gateway.setActivation(scopePlugin.id, { mode: 'per_agent', agentId: agent.agentId, enabled: !agent.enabled })
    setScope((current) => current.map((item) => item.agentId === agent.agentId ? { ...item, enabled: !item.enabled } : item))
    setScopePlugin(next)
    updateCard(next)
  }

  const handleInstalledToggle = async (card: PluginCardView, enabled: boolean) => {
    if (activationBusy.has(card.id) || card.activation.mode === 'not_applicable') return
    if (card.activation.mode === 'per_agent' && enabled) {
      await handleConfigure(card)
      return
    }
    setActivationBusy((current) => new Set(current).add(card.id))
    try {
      if (card.activation.mode === 'global') {
        await handleToggle(card, enabled)
      } else {
        const agents = await gateway.agentScopeList(card.id)
        let next = card
        for (const agent of agents.filter((item) => item.enabled)) {
          next = await gateway.setActivation(card.id, { mode: 'per_agent', agentId: agent.agentId, enabled: false })
        }
        updateCard(next)
      }
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : '无法更新插件启用状态。')
    } finally {
      setActivationBusy((current) => {
        const next = new Set(current)
        next.delete(card.id)
        return next
      })
    }
  }

  if (showInstalled) {
    return (
      <div className={`fox-plugin-center fox-plugin-installed-page ${className ?? ''}`}>
        <header className="fox-plugin-installed-page-header">
          <div className="fox-plugin-installed-page-title"><Button type="button" variant="ghost" className="fox-plugin-installed-back" onClick={() => setShowInstalled(false)}><ArrowLeft />全部技能</Button><h1>我安装的</h1><Badge variant="secondary">{installedCount}</Badge></div>
          <label className="fox-plugin-search" htmlFor="fox-plugin-installed-search-input"><Search /><Input id="fox-plugin-installed-search-input" value={installedSearch} onChange={(event) => setInstalledSearch(event.target.value)} placeholder="搜索已安装插件" /></label>
        </header>
        {error && <div className="fox-plugin-error"><CircleAlert /><span>{error}</span><Button type="button" variant="outline" size="sm" onClick={() => void reload()}>重试</Button></div>}
        <main className="fox-plugin-installed-content">{loading && !installed.length ? <div className="fox-plugin-loading"><LoaderCircle className="animate-spin" />正在加载已安装插件…</div> : installedCards.length === 0 ? <EmptyState>{installedSearch.trim() ? '没有匹配的已安装插件。' : '还没有安装插件。'}</EmptyState> : <div className="fox-plugin-installed-grid">{installedCards.map((card) => <InstalledPluginCard key={card.id} card={card} busy={activationBusy.has(card.id)} onToggle={(enabled) => void handleInstalledToggle(card, enabled)} />)}</div>}</main>
        {scopePlugin && <ScopeDialog plugin={scopePlugin} scope={scope} loading={scopeLoading} onClose={() => setScopePlugin(null)} onToggle={(agent) => void handleScopeToggle(agent)} />}
      </div>
    )
  }

  return (
    <div className={`fox-plugin-center ${className ?? ''}`}>
      <header className="fox-plugin-topbar">
        <Tabs value={kind} onValueChange={(value) => { if (kindOrder.includes(value as PluginKind)) { setKind(value as PluginKind); setCategory(undefined) } }}>
          <TabsList className="fox-plugin-kind-tabs">{kindOrder.map((item) => <TabsTrigger key={item} value={item}><KindIcon kind={item} />{pluginKindLabel(item)}</TabsTrigger>)}</TabsList>
        </Tabs>
        <div className="fox-plugin-topbar-actions">
          <label className="fox-plugin-search" htmlFor="fox-plugin-search-input"><Search /><Input id="fox-plugin-search-input" value={search} onChange={(event) => setSearch(event.target.value)} placeholder={`搜索${pluginKindLabel(kind)}`} /></label>
          <Button className="fox-plugin-toolbar-action" type="button" variant="outline" onClick={() => setShowInstalled(true)}><PackageCheck />我安装的<Badge variant="secondary">{installedCount}</Badge></Button>
          <Button className="fox-plugin-toolbar-action" type="button" onClick={onAddPlugin}><Plus />添加插件</Button>
        </div>
      </header>

      <div className="fox-plugin-filterbar">
        <div className="fox-plugin-categories" aria-label="插件分类"><button type="button" className={!category ? 'is-active' : ''} onClick={() => setCategory(undefined)}>全部</button>{categories.map((item) => <button type="button" key={item} className={category === item ? 'is-active' : ''} onClick={() => setCategory(item)}>{item}</button>)}</div>
        <Button className="fox-plugin-refresh-action" type="button" variant="ghost" onClick={() => void reload()} disabled={loading}><RefreshCw className={loading ? 'animate-spin' : ''} />刷新</Button>
      </div>

      {error && <div className="fox-plugin-error"><CircleAlert /><span>{error}</span><Button type="button" variant="outline" size="sm" onClick={() => void reload()}>重试</Button></div>}

      <main className="fox-plugin-content">
        {loading && !catalog.length ? <div className="fox-plugin-loading"><LoaderCircle className="animate-spin" />正在加载插件目录…</div> : visibleCards.length === 0 ? <EmptyState>没有找到匹配的{pluginKindLabel(kind)}。</EmptyState> : <>
          <section className="fox-plugin-section"><div className="fox-plugin-section-heading"><h2>精选{pluginKindLabel(kind)}</h2><Button type="button" variant="ghost" size="sm" onClick={() => { setSearch(''); setCategory(undefined) }}>换一换<RefreshCw /></Button></div><div className="fox-plugin-grid fox-plugin-featured-grid">{featuredCards.map((card) => <PluginCard key={card.id} card={card} onInstallAction={handleInstallAction} />)}</div></section>
          <section className="fox-plugin-section"><div className="fox-plugin-section-heading"><h2>全部{pluginKindLabel(kind)}</h2></div><div className="fox-plugin-grid">{visibleCards.map((card) => <PluginCard key={card.id} card={card} onInstallAction={handleInstallAction} />)}</div></section>
        </>}

        {operations.length > 0 && <section className="fox-plugin-operations"><div className="fox-plugin-section-heading"><h2>最近操作</h2></div><div className="fox-plugin-operation-list">{operations.slice(0, 4).map((operation) => <OperationRow key={operation.id} operation={operation} />)}</div></section>}
      </main>

      {scopePlugin && <ScopeDialog plugin={scopePlugin} scope={scope} loading={scopeLoading} onClose={() => setScopePlugin(null)} onToggle={(agent) => void handleScopeToggle(agent)} />}
    </div>
  )
}

export { PluginCard, ScopeDialog }
