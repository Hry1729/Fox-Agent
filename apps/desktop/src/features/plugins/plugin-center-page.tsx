import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react'
import {
  Cable,
  CircleAlert,
  CircleCheck,
  Database,
  File,
  FileText,
  Folder,
  Globe2,
  HardDrive,
  LoaderCircle,
  NotebookPen,
  PackageCheck,
  Puzzle,
  RefreshCw,
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
import '@/styles/plugin-center.css'
import { defaultPluginGateway } from './tauri-gateway'
import type { PluginAgentScopeDTO, PluginCardView, PluginGateway } from './model'
import {
  activationSummary,
  countInstalledPlugins,
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
  puzzle: Puzzle,
  sparkles: Sparkles,
  terminal: Terminal,
  wrench: Wrench,
}

export interface PluginCenterPageProps {
  gateway?: PluginGateway
  onConfigureAgent?: (plugin: PluginCardView) => void
  className?: string
}

function PluginIcon({ name }: { name?: string }) {
  const Icon = iconMap[name ?? 'puzzle'] ?? Puzzle
  return <Icon aria-hidden="true" />
}

function runtimeStatusLabel(status: PluginCardView['runtimeStatus']): string {
  if (status === 'ready' || status === 'healthy') return '运行正常'
  if (status === 'connecting') return '连接中'
  if (status === 'degraded') return '运行降级'
  if (status === 'error') return '运行异常'
  return '暂无运行状态'
}

function statusIcon(card: PluginCardView) {
  if (card.installStatus === 'installing' || card.installStatus === 'uninstalling') return <LoaderCircle className="animate-spin" />
  if (card.installStatus === 'install_failed' || card.runtimeStatus === 'error') return <CircleAlert />
  return <CircleCheck />
}

function StatusBadge({ card }: { card: PluginCardView }) {
  const statusClass = card.installStatus === 'installed'
    ? 'fox-plugin-status-installed'
    : card.installStatus === 'install_failed'
      ? 'fox-plugin-status-error'
      : 'fox-plugin-status-progress'
  return <Badge variant="outline" className={`fox-plugin-status ${statusClass}`}>{statusIcon(card)}{pluginInstallStatusLabel(card.installStatus)}</Badge>
}

function formatTimestamp(value?: number): string | null {
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) return null
  const date = new Date(value < 100_000_000_000 ? value * 1000 : value)
  return Number.isNaN(date.getTime()) ? null : date.toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' })
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
  const enabled = card.activation.mode === 'global'
    ? card.activation.enabled
    : card.activation.mode === 'per_agent'
      ? card.activation.enabledAgentCount > 0
      : true
  const isPerAgent = card.activation.mode === 'per_agent'
  const configurable = card.activation.mode !== 'not_applicable'
  const updatedAt = formatTimestamp(card.updatedAt)
  return (
    <Card className="fox-plugin-installed-card" data-plugin-id={card.id} data-install-status={card.installStatus}>
      <div className="fox-plugin-installed-card-header">
        <span className="fox-plugin-installed-card-icon"><PluginIcon name={card.icon} /></span>
        <span className="fox-plugin-installed-card-copy">
          <span className="fox-plugin-installed-card-title"><strong>{card.name}</strong><Badge variant="secondary">{pluginKindLabel(card.kind)}</Badge><Badge variant="outline">{pluginOriginLabel(card.origin)}</Badge></span>
          <small>{card.description}</small>
        </span>
        <StatusBadge card={card} />
      </div>
      <div className="fox-plugin-installed-card-meta">
        <span><small>来源</small><strong>{pluginOriginLabel(card.origin)}</strong></span>
        <span><small>版本</small><strong>{card.version ?? '未提供'}</strong></span>
        <span><small>运行时</small><strong className={card.runtimeStatus === 'error' ? 'is-error' : undefined}>{runtimeStatusLabel(card.runtimeStatus)}</strong></span>
        <span><small>权限</small><strong>{card.permissions.length > 0 ? card.permissions.join('、') : '未声明'}</strong></span>
      </div>
      {card.incompatibilityReason && <p className="fox-plugin-installed-card-error" role="alert"><CircleAlert />{card.incompatibilityReason}</p>}
      {card.lastError && card.lastError !== card.incompatibilityReason && <p className="fox-plugin-installed-card-error" role="alert"><CircleAlert />最近错误：{card.lastError}</p>}
      {card.runtimeStatus === 'error' && !card.incompatibilityReason && !card.lastError && <p className="fox-plugin-installed-card-error" role="alert"><CircleAlert />运行时报告异常，请检查 Host 日志。</p>}
      <div className="fox-plugin-installed-card-footer">
        <span className="fox-plugin-installed-card-activation"><PackageCheck />{activationSummary(card.activation)}{updatedAt && <small>更新于 {updatedAt}</small>}</span>
        {isPerAgent ? <Button type="button" variant="outline" size="sm" onClick={onConfigure} disabled={busy}><Settings2 />配置 Agent</Button> : configurable ? <label className="fox-plugin-installed-toggle"><span>{enabled ? '已启用' : '已停用'}</span><Switch checked={enabled} disabled={busy} onCheckedChange={onToggle} aria-label={`${enabled ? '停用' : '启用'}${card.name}`} size="sm" /></label> : <span className="fox-plugin-host-managed">由 Host 管理</span>}
      </div>
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
        {loading ? <div className="fox-plugin-scope-loading"><LoaderCircle className="animate-spin" />读取 Agent 配置中…</div> : scope.length === 0 ? <div className="fox-plugin-scope-loading">暂无可配置的 Agent。</div> : <div className="fox-plugin-scope-list">{scope.map((agent) => <label key={agent.agentId}><span><input type="checkbox" checked={agent.enabled} onChange={() => onToggle(agent)} /><b>{agent.agentName}</b></span><small>{agent.enabled ? '已启用' : '未启用'}</small></label>)}</div>}
        <footer><Button type="button" variant="outline" size="sm" onClick={onClose}>完成</Button></footer>
      </section>
    </div>
  )
}

function EmptyState({ children }: { children: ReactNode }) {
  return <div className="fox-plugin-empty"><Puzzle /><span>{children}</span></div>
}

export function PluginCenterPage({ gateway: providedGateway, onConfigureAgent, className }: PluginCenterPageProps) {
  const gateway = providedGateway ?? defaultPluginGateway
  const [installed, setInstalled] = useState<PluginCardView[]>([])
  const [installedSearch, setInstalledSearch] = useState('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [activationBusy, setActivationBusy] = useState<Set<string>>(new Set())
  const [scopePlugin, setScopePlugin] = useState<PluginCardView | null>(null)
  const [scope, setScope] = useState<PluginAgentScopeDTO[]>([])
  const [scopeLoading, setScopeLoading] = useState(false)

  const reload = useCallback(async () => {
    setLoading(true)
    try {
      const result = await gateway.installationsList()
      setInstalled(result.items)
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : '插件列表暂时无法加载。')
    } finally {
      setLoading(false)
    }
  }, [gateway])

  useEffect(() => { void reload() }, [reload])

  const installedCards = useMemo(() => {
    const normalizedSearch = installedSearch.trim().toLocaleLowerCase()
    return sortPluginCards(installed.filter((card) => !normalizedSearch || [card.id, card.name, card.description, card.category, pluginKindLabel(card.kind), pluginOriginLabel(card.origin)].some((value) => value.toLocaleLowerCase().includes(normalizedSearch))))
  }, [installed, installedSearch])

  const updateCard = (next: PluginCardView) => {
    setInstalled((current) => current.map((item) => item.id === next.id ? next : item))
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
  return (
    <div className={`fox-plugin-center fox-plugin-installed-page ${className ?? ''}`}>
      <header className="fox-plugin-installed-page-header">
        <div className="fox-plugin-installed-page-title"><span className="fox-plugin-installed-page-icon"><PackageCheck /></span><div><h1>插件</h1><p>管理已安装插件及其运行状态。</p></div><Badge variant="secondary">{installedCount}</Badge></div>
        <div className="fox-plugin-installed-page-actions"><label className="fox-plugin-search" htmlFor="fox-plugin-installed-search-input"><Puzzle /><Input id="fox-plugin-installed-search-input" value={installedSearch} onChange={(event) => setInstalledSearch(event.target.value)} placeholder="搜索已安装插件" /></label><Button type="button" variant="outline" onClick={() => void reload()} disabled={loading}><RefreshCw className={loading ? 'animate-spin' : undefined} />刷新</Button></div>
      </header>
      {error && <div className="fox-plugin-error"><CircleAlert /><span>{error}</span><Button type="button" variant="outline" size="sm" onClick={() => void reload()}>重试</Button></div>}
      <main className="fox-plugin-installed-content">
        {loading && !installed.length ? <div className="fox-plugin-loading"><LoaderCircle className="animate-spin" />正在加载已安装插件…</div> : installedCards.length === 0 ? <EmptyState>{installedSearch.trim() ? '没有匹配的已安装插件。' : '暂无已安装插件。'}</EmptyState> : <div className="fox-plugin-installed-grid">{installedCards.map((card) => <InstalledPluginCard key={card.id} card={card} busy={activationBusy.has(card.id)} onToggle={(enabled) => void handleToggle(card, enabled)} onConfigure={() => void handleConfigure(card)} />)}</div>}
      </main>
      {scopePlugin && <ScopeDialog plugin={scopePlugin} scope={scope} loading={scopeLoading} onClose={() => setScopePlugin(null)} onToggle={(agent) => void handleScopeToggle(agent)} />}
    </div>
  )
}

export { InstalledPluginCard, InstalledPluginCard as PluginCard, ScopeDialog }
