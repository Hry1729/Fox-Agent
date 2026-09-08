import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react'
import {
  Cable,
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
  PackagePlus,
  Plus,
  RefreshCw,
  Search,
  Settings2,
  Terminal,
  WandSparkles,
  Wrench,
  X,
  type LucideIcon,
} from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import '@/styles/plugin-center.css'
import { defaultPluginGateway } from './tauri-gateway'
import type { PluginAgentScopeDTO, PluginCardView, PluginGateway, PluginKind } from './model'
import {
  countInstalledPlugins,
  filterPluginCards,
  pluginInstallAction,
  pluginInstallStatusLabel,
  pluginKindLabel,
  pluginOriginLabel,
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
  package: PackagePlus,
  'package-plus': PackagePlus,
  puzzle: PackagePlus,
  sparkles: WandSparkles,
  terminal: Terminal,
  wrench: Wrench,
}

const kindIcons: Record<PluginKind, LucideIcon> = {
  tool: Wrench,
  skill: WandSparkles,
  mcp: Cable,
}

const kindOrder: PluginKind[] = ['tool', 'skill', 'mcp']

export interface PluginCenterPageProps {
  gateway?: PluginGateway
  initialKind?: PluginKind
  onAddPlugin?: () => void
  onConfigureAgent?: (plugin: PluginCardView) => void
  className?: string
}

function PluginIcon({ name }: { name?: string }) {
  const Icon = iconMap[name ?? 'package-plus'] ?? PackagePlus
  return <Icon aria-hidden="true" />
}

function KindIcon({ kind }: { kind: PluginKind }) {
  const Icon = kindIcons[kind]
  return <Icon aria-hidden="true" />
}

function statusIcon(card: PluginCardView) {
  if (card.installStatus === 'installing' || card.installStatus === 'uninstalling') return <LoaderCircle className="animate-spin" />
  if (card.installStatus === 'install_failed' || card.runtimeStatus === 'error') return <CircleAlert />
  if (card.installStatus === 'not_installed' || card.installStatus === 'update_available') return <Download />
  return <CircleCheck />
}

function StatusBadge({ card }: { card: PluginCardView }) {
  const statusClass = card.installStatus === 'installed'
    ? 'fox-plugin-status-installed'
    : card.installStatus === 'install_failed' || card.runtimeStatus === 'error'
      ? 'fox-plugin-status-error'
      : card.installStatus === 'not_installed' || card.installStatus === 'update_available'
        ? 'fox-plugin-status-update'
        : 'fox-plugin-status-progress'
  return <Badge variant="outline" className={`fox-plugin-status ${statusClass}`}>{statusIcon(card)}{pluginInstallStatusLabel(card.installStatus)}</Badge>
}

function CatalogPluginCard({
  card,
  onInstallAction,
}: {
  card: PluginCardView
  onInstallAction: (card: PluginCardView) => void
}) {
  const action = pluginInstallAction(card)
  const canInstall = action.action === 'install' || action.action === 'update'
  return (
    <Card className="fox-library-card fox-library-card--stacked-meta fox-shadcn-kb-card fox-shadcn-agent-card fox-plugin-card" data-plugin-id={card.id} data-install-status={card.installStatus}>
      <CardHeader className="fox-shadcn-kb-head fox-plugin-card-heading">
        <span className="fox-shadcn-kb-icon fox-shadcn-agent-icon fox-plugin-icon"><PluginIcon name={card.icon} /></span>
        <span className="fox-shadcn-kb-heading fox-plugin-card-title"><strong>{card.name}</strong></span>
      </CardHeader>
      <span className="fox-plugin-card-state">{canInstall
        ? <Button className="fox-plugin-card-install" type="button" variant="outline" size="sm" disabled={action.disabled} title={card.incompatibilityReason} onClick={() => onInstallAction(card)}><Download />{action.label}</Button>
        : <StatusBadge card={card} />}</span>
      <CardContent className="fox-shadcn-kb-content fox-plugin-card-content">
        <p className={`fox-agent-description ${card.incompatibilityReason ? 'fox-plugin-card-error' : ''}`} title={card.incompatibilityReason ?? card.description}>{card.incompatibilityReason ?? card.description}</p>
        <span className="fox-library-card-tags"><small>{pluginOriginLabel(card.origin)}</small><small>{pluginKindLabel(card.kind)}</small></span>
      </CardContent>
    </Card>
  )
}

function InstalledPluginCard({
  card,
  busy,
  onToggle,
  onConfigure,
}: {
  card: PluginCardView
  busy: boolean
  onToggle: (enabled: boolean) => void
  onConfigure: () => void
}) {
  void busy
  void onToggle
  void onConfigure
  const issue = card.incompatibilityReason
    ?? card.lastError
    ?? (card.runtimeStatus === 'error' ? '运行时报告异常，请检查 Host 日志。' : undefined)
  return (
    <Card className="fox-library-card fox-library-card--stacked-meta fox-shadcn-kb-card fox-shadcn-agent-card fox-plugin-installed-card" data-plugin-id={card.id} data-install-status={card.installStatus}>
      <CardHeader className="fox-shadcn-kb-head fox-plugin-installed-card-header">
        <span className="fox-shadcn-kb-icon fox-shadcn-agent-icon fox-plugin-installed-card-icon"><PluginIcon name={card.icon} /></span>
        <span className="fox-shadcn-kb-heading fox-plugin-installed-card-copy">
          <span className="fox-plugin-installed-card-title"><strong>{card.name}</strong></span>
        </span>
      </CardHeader>
      <CardContent className="fox-shadcn-kb-content fox-plugin-card-content">
        <p className={`fox-agent-description ${issue ? 'fox-plugin-installed-card-error' : ''}`} role={issue ? 'alert' : undefined} title={issue ?? card.description}>{issue && <CircleAlert />}{issue ?? card.description}</p>
        <span className="fox-library-card-tags"><small>{pluginOriginLabel(card.origin)}</small><small>{pluginKindLabel(card.kind)}</small></span>
      </CardContent>
    </Card>
  )
}

function ScopeDialog({
  plugin,
  scope,
  loading,
  onClose,
  onToggle,
}: {
  plugin: PluginCardView
  scope: PluginAgentScopeDTO[]
  loading: boolean
  onClose: () => void
  onToggle: (agent: PluginAgentScopeDTO) => void
}) {
  return (
    <div className="fox-plugin-scope-backdrop" role="presentation" onMouseDown={(event) => { if (event.currentTarget === event.target) onClose() }}>
      <section className="fox-plugin-scope-dialog" role="dialog" aria-modal="true" aria-labelledby="fox-plugin-scope-title">
        <header><span><Settings2 /></span><div><h2 id="fox-plugin-scope-title">配置启用范围</h2><p>{plugin.name} 只对选定的 Agent 生效。</p></div><Button type="button" variant="ghost" size="icon" aria-label="关闭配置范围" onClick={onClose}><X /></Button></header>
        {loading
          ? <div className="fox-plugin-scope-loading"><LoaderCircle className="animate-spin" />读取 Agent 配置中…</div>
          : scope.length === 0
            ? <div className="fox-plugin-scope-loading">暂无可配置的 Agent。</div>
            : <div className="fox-plugin-scope-list">{scope.map((agent) => <label key={agent.agentId}><span><input type="checkbox" checked={agent.enabled} onChange={() => onToggle(agent)} /><b>{agent.agentName}</b></span><small>{agent.enabled ? '已启用' : '未启用'}</small></label>)}</div>}
        <footer><Button type="button" variant="outline" size="sm" onClick={onClose}>完成</Button></footer>
      </section>
    </div>
  )
}

function EmptyState({ children }: { children: ReactNode }) {
  return <div className="fox-plugin-empty"><PackagePlus /><span>{children}</span></div>
}

export function PluginCenterPage({ gateway: providedGateway, initialKind, onAddPlugin, onConfigureAgent, className }: PluginCenterPageProps) {
  const gateway = providedGateway ?? defaultPluginGateway
  const [kind, setKind] = useState<PluginKind>(initialKind && kindOrder.includes(initialKind) ? initialKind : 'tool')
  const [search, setSearch] = useState('')
  const [category, setCategory] = useState<string | undefined>()
  const [catalog, setCatalog] = useState<PluginCardView[]>([])
  const [installed, setInstalled] = useState<PluginCardView[]>([])
  const [installedSearch, setInstalledSearch] = useState('')
  const [categories, setCategories] = useState<string[]>([])
  // Host 尚未提供包校验、依赖安装、回滚与清理命令前，正式页面只暴露已安装插件。
  const showInstalled = true
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [activationBusy, setActivationBusy] = useState<Set<string>>(new Set())
  const [scopePlugin, setScopePlugin] = useState<PluginCardView | null>(null)
  const [scope, setScope] = useState<PluginAgentScopeDTO[]>([])
  const [scopeLoading, setScopeLoading] = useState(false)

  useEffect(() => {
    if (initialKind && kindOrder.includes(initialKind)) {
      setKind(initialKind)
      setCategory(undefined)
    }
  }, [initialKind])

  const reload = useCallback(async () => {
    setLoading(true)
    try {
      const [catalogResult, installedResult] = await Promise.all([
        gateway.catalogList({ kind, search, pageSize: 50 }),
        gateway.installationsList(),
      ])
      setCatalog(catalogResult.items)
      setCategories(catalogResult.categories)
      setInstalled(installedResult.items)
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : '插件中心暂时无法加载。')
    } finally {
      setLoading(false)
    }
  }, [gateway, kind, search])

  useEffect(() => { void reload() }, [reload])

  const featuredCards = useMemo(
    () => sortPluginCards(filterPluginCards(catalog, { kind, search })).slice(0, 3),
    [catalog, kind, search],
  )
  const visibleCards = useMemo(
    () => sortPluginCards(filterPluginCards(catalog, { kind, search, category })),
    [catalog, category, kind, search],
  )
  const installedCards = useMemo(() => {
    const normalizedSearch = installedSearch.trim().toLocaleLowerCase()
    return sortPluginCards(installed.filter((card) => card.kind === kind && (!normalizedSearch || [card.id, card.name, card.description, card.category, pluginKindLabel(card.kind), pluginOriginLabel(card.origin)].some((value) => value.toLocaleLowerCase().includes(normalizedSearch)))))
  }, [installed, installedSearch, kind])

  const updateCard = (next: PluginCardView) => {
    setCatalog((current) => current.map((item) => item.id === next.id ? next : item))
    setInstalled((current) => current.map((item) => item.id === next.id ? next : item))
  }

  const handleInstallAction = async (card: PluginCardView) => {
    const action = pluginInstallAction(card).action
    if (action !== 'install' && action !== 'update') return
    try {
      if (action === 'install') await gateway.install(card.id)
      else await gateway.update(card.id)
      await reload()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : '无法完成插件安装操作。')
    }
  }

  const handleConfigure = async (card: PluginCardView) => {
    onConfigureAgent?.(card)
    setScopePlugin(card)
    setScope([])
    setScopeLoading(true)
    try {
      setScope(await gateway.agentScopeList(card.id))
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : '无法读取 Agent 启用范围。')
    } finally {
      setScopeLoading(false)
    }
  }

  const handleScopeToggle = async (agent: PluginAgentScopeDTO) => {
    if (!scopePlugin) return
    try {
      const next = await gateway.setActivation(scopePlugin.id, { mode: 'per_agent', agentId: agent.agentId, enabled: !agent.enabled })
      setScope((current) => current.map((item) => item.agentId === agent.agentId ? { ...item, enabled: !agent.enabled } : item))
      setScopePlugin(next)
      updateCard(next)
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : '无法更新插件启用状态。')
    }
  }

  const handleToggle = async (card: PluginCardView, enabled: boolean) => {
    if (activationBusy.has(card.id) || card.activation.mode === 'not_applicable') return
    setActivationBusy((current) => new Set(current).add(card.id))
    try {
      if (card.activation.mode === 'global') {
        updateCard(await gateway.setActivation(card.id, { mode: 'global', enabled }))
      } else if (card.activation.mode === 'per_agent') {
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

  const installedCount = countInstalledPlugins(installed)
  if (showInstalled) {
    return (
      <div className={`fox-plugin-center fox-plugin-installed-page ${className ?? ''}`}>
        <header className="fox-plugin-installed-page-header">
          <h1 className="sr-only">插件</h1>
          <Tabs value={kind} onValueChange={(value) => { if (kindOrder.includes(value as PluginKind)) { setKind(value as PluginKind); setCategory(undefined) } }}>
            <TabsList className="fox-plugin-kind-tabs">{kindOrder.map((item) => <TabsTrigger key={item} value={item}><KindIcon kind={item} />{pluginKindLabel(item)}</TabsTrigger>)}</TabsList>
          </Tabs>
          <label className="fox-plugin-search" htmlFor="fox-plugin-installed-search-input"><Search /><Input id="fox-plugin-installed-search-input" value={installedSearch} onChange={(event) => setInstalledSearch(event.target.value)} placeholder={`搜索${pluginKindLabel(kind)}`} /></label>
        </header>
        {error && <div className="fox-plugin-error"><CircleAlert /><span>{error}</span><Button type="button" variant="outline" size="sm" onClick={() => void reload()}>重试</Button></div>}
        <main className="fox-plugin-installed-content">
          {loading && !installed.length
            ? <div className="fox-plugin-loading"><LoaderCircle className="animate-spin" />正在加载已安装插件…</div>
            : installedCards.length === 0
              ? <EmptyState>{installedSearch.trim() ? `没有匹配的${pluginKindLabel(kind)}。` : `暂无已安装${pluginKindLabel(kind)}。`}</EmptyState>
              : <div className="fox-plugin-installed-grid">{installedCards.map((card) => <InstalledPluginCard key={card.id} card={card} busy={activationBusy.has(card.id)} onToggle={(enabled) => void handleToggle(card, enabled)} onConfigure={() => void handleConfigure(card)} />)}</div>}
        </main>
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
          <Button className="fox-plugin-toolbar-action" type="button" variant="outline" onClick={() => void reload()}><RefreshCw />刷新<Badge variant="secondary">{installedCount}</Badge></Button>
          {onAddPlugin && <Button className="fox-plugin-toolbar-action" type="button" onClick={onAddPlugin}><Plus />添加插件</Button>}
        </div>
      </header>

      {error && <div className="fox-plugin-error"><CircleAlert /><span>{error}</span><Button type="button" variant="outline" size="sm" onClick={() => void reload()}>重试</Button></div>}

      <main className="fox-plugin-content">
        {loading && !catalog.length
          ? <div className="fox-plugin-loading"><LoaderCircle className="animate-spin" />正在加载插件目录…</div>
          : catalog.length === 0
            ? <EmptyState>没有找到匹配的{pluginKindLabel(kind)}。</EmptyState>
            : <>
              <section className="fox-plugin-section fox-plugin-featured-section">
                <div className="fox-plugin-section-heading"><h2>精选{pluginKindLabel(kind)}</h2></div>
                <div className="fox-plugin-grid fox-plugin-featured-grid">{featuredCards.map((card) => <CatalogPluginCard key={card.id} card={card} onInstallAction={(item) => void handleInstallAction(item)} />)}</div>
              </section>
              <section className="fox-plugin-section fox-plugin-all-section">
                <div className="fox-plugin-section-heading"><h2>全部{pluginKindLabel(kind)}</h2></div>
                <div className="fox-plugin-filterbar">
                  <div className="fox-plugin-categories" aria-label="插件分类"><button type="button" className={!category ? 'is-active' : ''} onClick={() => setCategory(undefined)}>全部</button>{categories.map((item) => <button type="button" key={item} className={category === item ? 'is-active' : ''} onClick={() => setCategory(item)}>{item}</button>)}</div>
                  <Button className="fox-plugin-refresh-action" type="button" variant="ghost" onClick={() => void reload()} disabled={loading}><RefreshCw className={loading ? 'animate-spin' : undefined} />刷新</Button>
                </div>
                {visibleCards.length === 0
                  ? <EmptyState>没有找到匹配的{pluginKindLabel(kind)}。</EmptyState>
                  : <div className="fox-plugin-grid">{visibleCards.map((card) => <CatalogPluginCard key={card.id} card={card} onInstallAction={(item) => void handleInstallAction(item)} />)}</div>}
              </section>
            </>}
      </main>

      {scopePlugin && <ScopeDialog plugin={scopePlugin} scope={scope} loading={scopeLoading} onClose={() => setScopePlugin(null)} onToggle={(agent) => void handleScopeToggle(agent)} />}
    </div>
  )
}

export { CatalogPluginCard as PluginCard, InstalledPluginCard, ScopeDialog }
