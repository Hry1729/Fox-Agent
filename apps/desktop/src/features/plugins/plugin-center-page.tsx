import { useEffect, useRef, useState } from 'react'
import { ChevronDown, ChevronRight, Plus, Search, Settings2, ShieldCheck } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { FoxEmptyState } from '@/components/fox-ui/empty-state'
import { SkeletonContent } from '@/components/ui/skeleton'
import { ConnectorSetupSheet } from '@/features/settings/connector-setup-sheet'
import { SearchServiceSheet } from '@/features/settings/search-service-page'
import { useMcpServers } from '@/features/settings/use-mcp-servers'
import type { NavigateWorkspace } from '@/features/workspace/types'
import { defaultPluginGateway } from './tauri-gateway'
import type { PluginCardView, PluginGateway } from './model'
import { matchesPluginQuery, pluginSection, pluginSectionLabels, pluginStatus, usePluginEntries, type PluginSection } from './plugin-browser-data'
import { PluginMark, PluginPolicySheet, PluginStatusBadge, PluginTabs, SkillImportDialog, openPluginDetail } from './plugin-controls'
import { sortPluginCards } from './selectors'
import { CatalogConnectSheet } from './catalog-connect-sheet'
import { connectorCatalog, findConnectorPreset, type ConnectorPreset } from './connector-presets'

export type PluginCenterTab = PluginSection | 'policy'

export interface PluginCenterPageProps {
  gateway?: PluginGateway
  initialKind?: PluginSection
  initialCategory?: string
  initialTab?: PluginCenterTab
  navigate?: NavigateWorkspace
  onAddPlugin?: () => void
  onConfigureAgent?: (plugin: PluginCardView) => void
  className?: string
}

const statusOptions = [
  ['all', '全部状态'], ['available', '可用'], ['disabled', '未启用'], ['unconfigured', '未配置'], ['error', '异常'],
] as const
type DialogKind = 'mcp' | 'plugin' | 'skill' | 'policy'
const primaryActions = {
  mcp: { label: '添加连接器', Icon: Plus, dialog: 'mcp' },
  plugin: { label: '管理搜索服务', Icon: Settings2, dialog: 'plugin' },
  skill: { label: '导入技能', Icon: Plus, dialog: 'skill' },
} as const

const featuredIds = ['github', 'feishu', 'linear', 'drive', 'wechat', 'qq'].map(id => 'mcp:fox-catalog-' + id)

export function PluginCenterPage({
  gateway = defaultPluginGateway, initialKind, initialCategory, initialTab, navigate, onAddPlugin, onConfigureAgent, className,
}: PluginCenterPageProps) {
  const startingSection = initialKind ?? (initialTab && initialTab !== 'policy' ? initialTab : 'mcp')
  const [kind, setKind] = useState<PluginSection>(startingSection)
  const [category, setCategory] = useState(initialCategory ?? '')
  const [status, setStatus] = useState('all')
  const [search, setSearch] = useState('')
  const [dialog, setDialog] = useState<DialogKind | null>(initialTab === 'policy' ? 'policy' : null)
  const [expanded, setExpanded] = useState<string[]>([])
  const [connecting, setConnecting] = useState<ConnectorPreset | null>(null)
  const searchInput = useRef<HTMLInputElement>(null)
  const entries = usePluginEntries(gateway, kind)
  const mcp = useMcpServers(dialog === 'mcp' || Boolean(connecting))
  useEffect(() => {
    const focusSearch = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k' && !dialog && !connecting) {
        event.preventDefault()
        event.stopPropagation()
        searchInput.current?.focus()
      }
    }
    window.addEventListener('keydown', focusSearch, true)
    return () => window.removeEventListener('keydown', focusSearch, true)
  }, [dialog, connecting])
  useEffect(() => { setKind(startingSection); setCategory(initialCategory ?? '') }, [startingSection, initialCategory])
  const cards = entries.items.filter(card => pluginSection(card) === kind)
  const categorySet = new Set(cards.map(card => card.category))
  const categories = kind === 'mcp'
    ? [...new Set(['即时通讯', ...connectorCatalog.map(item => item.category), ...categorySet])].filter(name => categorySet.has(name))
    : [...categorySet]
  const visible = sortPluginCards(cards.filter(card => (!category || card.category === category)
    && (status === 'all' || status === pluginStatus(card)) && matchesPluginQuery(card, search)))
  const overview = kind === 'mcp' && !category && status === 'all' && !search.trim()
  const featured = overview ? featuredIds.flatMap(id => visible.find(card => card.id === id) ?? []) : []
  const remaining = visible.filter(card => !featured.includes(card))
  const configured = overview ? visible.filter(card => card.installStatus === 'installed') : []
  const byCategory = remaining.filter(card => !configured.includes(card))
  const groups = [
    { name: '优先推荐', cards: featured }, { name: '已配置', cards: configured },
    ...categories.map(name => ({ name, cards: byCategory.filter(card => card.category === name) })),
  ].filter(group => group.cards.length > 0)
  const action = primaryActions[kind]
  const closeDialog = () => setDialog(null)
  return <div className={`fox-plugin-center fox-extension-directory ${className ?? ''}`}>
    <header className="fox-extension-page-head">
      <h1>{kind === 'mcp' ? '连接器目录' : pluginSectionLabels[kind]}</h1>
      <div>
        <Button variant="outline" onClick={() => setDialog('policy')}><ShieldCheck />插件策略</Button>
        <Button onClick={() => {
          if (kind === 'mcp' && onAddPlugin) onAddPlugin()
          else setDialog(action.dialog)
        }}><action.Icon />{action.label}</Button>
      </div>
    </header>
    <div className="fox-extension-toolbar">
      <PluginTabs section={kind} onChange={next => {
        setKind(next); setCategory(''); setStatus('all'); setSearch(''); setExpanded([])
        navigate?.('plugins', next)
      }} />
      <div className="fox-extension-filters">
        <Select value={category ? `category:${category}` : 'all'} onValueChange={value => setCategory(value === 'all' ? '' : value.slice(9))}>
          <SelectTrigger aria-label="分类" size="sm"><SelectValue /></SelectTrigger>
          <SelectContent><SelectItem value="all">全部分类</SelectItem>{categories.map(name => <SelectItem key={name} value={`category:${name}`}>{name}</SelectItem>)}</SelectContent>
        </Select>
        <Select value={status} onValueChange={setStatus}><SelectTrigger aria-label="状态" size="sm"><SelectValue /></SelectTrigger>
          <SelectContent>{statusOptions.map(([id, label]) => <SelectItem key={id} value={id}>{label}</SelectItem>)}</SelectContent>
        </Select>
        <label className="fox-extension-search"><Search size={14} /><Input ref={searchInput} type="search" aria-label={'搜索' + pluginSectionLabels[kind]} placeholder={'搜索' + pluginSectionLabels[kind]} value={search} onChange={event => setSearch(event.target.value)} /><kbd>Ctrl K</kbd></label>
      </div>
    </div>
    {entries.error && <FoxEmptyState variant="error" title="无法加载插件目录" description={entries.error} action={<Button variant="outline" onClick={entries.refresh}>重试</Button>} />}
    {entries.loading && !cards.length && <SkeletonContent label="正在加载插件目录" lines={4} avatar />}
    {!entries.loading && !entries.error && !groups.length && <FoxEmptyState variant="search" title={cards.length ? '没有匹配的条目' : `暂无${pluginSectionLabels[kind]}`}
      action={cards.length ? <Button variant="outline" onClick={() => { setCategory(''); setStatus('all'); setSearch('') }}>清除筛选</Button> : undefined} />}
    <div className="fox-extension-directory-content">
      {groups.map(group => {
        const isExpanded = expanded.includes(group.name)
        return <section className="fox-extension-group" key={group.name}>
          <header><h2>{group.name} <small>{group.cards.length} 项</small></h2>
            {group.cards.length > 6 && <Button variant="ghost" size="sm" aria-expanded={isExpanded} onClick={() => setExpanded(items => isExpanded ? items.filter(name => name !== group.name) : [...items, group.name])}>
              {isExpanded ? '收起' : `查看全部 ${group.cards.length} 项`}{isExpanded ? <ChevronDown /> : <ChevronRight />}
            </Button>}
          </header>
          <div className="fox-extension-grid">{(isExpanded ? group.cards : group.cards.slice(0, 6)).map(card => <PluginCard key={card.id} card={card} onConnect={setConnecting}
            onOpen={navigate ? () => openPluginDetail(navigate, card) : onConfigureAgent ? () => onConfigureAgent(card) : undefined} />)}</div>
        </section>
      })}
    </div>
    {connecting && <CatalogConnectSheet item={connecting} mcp={mcp} onClose={() => setConnecting(null)} onChange={entries.refresh} />}
    {dialog === 'mcp' && <ConnectorSetupSheet mcp={mcp} onClose={closeDialog} onSaved={entries.refresh} />}
    <SearchServiceSheet open={dialog === 'plugin'} onOpenChange={open => { if (!open) closeDialog() }} onSaved={entries.refresh} />
    <SkillImportDialog open={dialog === 'skill'} onOpenChange={open => { if (!open) closeDialog() }} onRescan={entries.refresh} />
    <PluginPolicySheet open={dialog === 'policy'} onOpenChange={open => { if (!open) closeDialog() }} navigate={navigate} />
  </div>
}

export function PluginCard({ card, onOpen, onConnect }: { card: PluginCardView; onOpen?: () => void; onConnect?: (item: ConnectorPreset) => void }) {
  const preset = findConnectorPreset(card.id)
  const quickConnect = preset && onConnect && card.installStatus === 'not_installed'
  return <div className="fox-directory-entry">
    <button type="button" className="fox-extension-card" data-plugin-id={card.id} onClick={onOpen} disabled={!onOpen}>
      <PluginMark kind={pluginSection(card)} name={card.icon} id={card.id} />
      <span className="fox-extension-card-copy"><span className="fox-directory-entry-title"><strong>{card.name}</strong>
        {(!quickConnect || pluginStatus(card) === 'error') && <PluginStatusBadge card={card} />}</span>
        <small>{card.description}</small></span>
      <ChevronRight className="fox-directory-chevron" size={14} aria-hidden="true" />
    </button>
    {quickConnect && <button type="button" className="fox-directory-connect" aria-label={'连接' + card.name} title={'连接' + card.name} onClick={() => onConnect(preset)}><Plus size={15} /></button>}
  </div>
}
