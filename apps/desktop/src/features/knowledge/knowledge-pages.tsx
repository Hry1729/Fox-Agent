import { useEffect, useMemo, useRef, useState } from 'react'
import { Bot, ChevronRight, Copy, Download, FileText, FolderOpen, Link2, ListTree, LoaderCircle, LogIn, Maximize2, MessageSquare, Minus, MonitorUp, MoreHorizontal, Network, Plus, RotateCcw, Search, Server, Sparkles, X } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { KnowledgeBaseRecord, KnowledgeDocumentRecord } from '@/features/conversations/model/types'
import { KnowledgeCard, type KnowledgeCardData } from '@/features/workspace/entity-card'
import { WorkspacePage } from '@/features/workspace/page-shell'
import type { KnowledgeSourceLocator, NavigateWorkspace } from '@/features/workspace/types'
import { useYuxiService } from '@/features/settings/use-yuxi-service'
import { useYuxiUser } from '@/features/settings/use-yuxi-user'
import { useKnowledgeBases, useKnowledgeDetail } from './use-knowledge'
import { KnowledgeFileViewer } from './knowledge-file-viewer'
import { formatKnowledgeDocumentInfo } from './knowledge-preview-model'
import { knowledgeDocumentIcon } from './knowledge-resource-explorer'
import { openKnowledgeCachedPreviewSource } from './knowledge-preview-source'
import { KnowledgeGraphCanvas, type KnowledgeGraphCanvasHandle } from './knowledge-graph-canvas'
import { YUXI_LOGIN_RETURN_KEY, YUXI_SERVICE_RETURN_KEY } from './knowledge-navigation'

const MAX_LOCAL_OPEN_BYTES = 500 * 1024 * 1024

function asCard(item: KnowledgeBaseRecord, index: number): KnowledgeCardData {
  const progress = item.fileCount > 0 ? Math.round(item.processedCount / item.fileCount * 100) : item.status === 'active' ? 100 : 0
  return { ...item, types: item.kbType || '知识库', progress, agents: 0, recent: item.updatedAt ? new Date(item.updatedAt).toLocaleDateString() : '来自知识库服务', tone: ['blue', 'violet', 'green', 'amber', 'gray'][index % 5] }
}

function contentText(value: unknown): string {
  if (typeof value === 'string') return value
  if (Array.isArray(value)) return value.map(contentText).join('\n')
  if (value && typeof value === 'object') {
    const object = value as Record<string, unknown>
    for (const key of ['content', 'markdown', 'text', 'data']) if (object[key] !== undefined) return contentText(object[key])
  }
  return value ? JSON.stringify(value, null, 2) : ''
}

export function KnowledgeListPage({ sidebarCollapsed, onSidebar, navigate }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace }) {
  const yuxi = useYuxiService()
  const yuxiUser = useYuxiUser(Boolean(yuxi.service?.credentialConfigured))
  const resource = useKnowledgeBases(Boolean(yuxiUser.user))
  const [query, setQuery] = useState('')
  const [statusFilter, setStatusFilter] = useState('all')
  const [typeFilter, setTypeFilter] = useState('all')
  const [contentFilter, setContentFilter] = useState('all')
  const normalizedQuery = query.trim().toLocaleLowerCase()
  const allCards = resource.items.map(asCard)
  const typeOptions = Array.from(new Set(allCards.map((item) => item.kbType || '知识库')))
  const cards = allCards
    .filter((item) => !normalizedQuery || [item.name, item.description, item.kbType]
      .some((value) => value?.toLocaleLowerCase().includes(normalizedQuery)))
    .filter((item) => statusFilter === 'all' || (statusFilter === 'ready' ? item.progress >= 100 : statusFilter === 'processing' ? item.progress > 0 && item.progress < 100 : item.progress === 0))
    .filter((item) => typeFilter === 'all' || (item.kbType || '知识库') === typeFilter)
    .filter((item) => contentFilter === 'all' || (contentFilter === 'populated' ? item.fileCount > 0 : item.fileCount === 0))
  const connected = yuxi.service?.lastStatus === 'connected'
  const openService = () => {
    window.sessionStorage.setItem(YUXI_SERVICE_RETURN_KEY, 'knowledge')
    navigate('service')
  }
  const openLogin = () => {
    window.sessionStorage.setItem(YUXI_LOGIN_RETURN_KEY, 'knowledge')
    navigate('login')
  }
  return (
    <WorkspacePage title="知识库" subtitle="浏览你有权限访问的资料" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
      <section className="fox-page-content fox-knowledge-list-page">
        <div className="fox-page-intro fox-list-page-intro"><h1>知识库</h1><label className="fox-list-page-search"><Search size={14} /><Input aria-label="搜索知识库" placeholder="搜索知识库..." value={query} onChange={(event) => setQuery(event.target.value)} /></label></div>
        <div className="fox-page-toolbar fox-agent-toolbar fox-knowledge-toolbar">
          <Select value={statusFilter} onValueChange={setStatusFilter}><SelectTrigger className="fox-agent-filter"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">全部状态</SelectItem><SelectItem value="ready">已就绪</SelectItem><SelectItem value="processing">处理中</SelectItem><SelectItem value="pending">待处理</SelectItem></SelectContent></Select>
          <Select value={typeFilter} onValueChange={setTypeFilter}><SelectTrigger className="fox-agent-filter"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">全部类型</SelectItem>{typeOptions.map((type) => <SelectItem key={type} value={type}>{type}</SelectItem>)}</SelectContent></Select>
          <Select value={contentFilter} onValueChange={setContentFilter}><SelectTrigger className="fox-agent-filter"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">全部内容</SelectItem><SelectItem value="populated">包含文件</SelectItem><SelectItem value="empty">空知识库</SelectItem></SelectContent></Select>
          <span className="fox-agent-result-count">{cards.length} 个结果</span>
          <Button variant="ghost" size="sm" onClick={() => void resource.refresh()}>刷新</Button>
        </div>
        {resource.error && <p className="fox-page-error">{resource.error}</p>}
        {!yuxi.loading && !yuxi.service && <div className="fox-page-empty-state"><Server /><span><b>尚未配置知识库服务</b><small>先连接知识库服务，再登录账户访问资料。</small></span><Button size="lg" onClick={openService}>配置服务</Button></div>}
        {!yuxi.loading && yuxi.service && !yuxiUser.loading && !yuxiUser.user && <div className="fox-page-empty-state"><LogIn /><span><b>{connected ? '知识库服务已连接，账户尚未登录' : '需要登录知识库'}</b><small>{yuxiUser.error ?? '浏览器中的登录态不会与 Fox 自动共享，请在 Fox 中登录一次。'}</small></span><Button size="lg" onClick={openLogin}>知识库登录</Button></div>}
        {!resource.loading && yuxiUser.user && resource.items.length === 0 && <p className="fox-page-empty">当前账户暂无可访问的知识库。</p>}
        {!resource.loading && yuxiUser.user && resource.items.length > 0 && cards.length === 0 && <p className="fox-page-empty">没有匹配“{query.trim()}”的知识库。</p>}
        <div className="fox-entity-grid fox-shadcn-entity-grid fox-knowledge-grid">{cards.map((knowledge) => <KnowledgeCard key={knowledge.id} knowledge={knowledge} onOpen={() => navigate('knowledge-detail', knowledge.id)} />)}</div>
      </section>
    </WorkspacePage>
  )
}

export function KnowledgeDetailPage({ sidebarCollapsed, onSidebar, navigate, onAskKnowledge, knowledgeId, initialDocumentId, sourceLocator }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace; onAskKnowledge: (knowledgeBase: KnowledgeBaseRecord) => void; knowledgeId?: string | null; initialDocumentId?: string | null; sourceLocator?: KnowledgeSourceLocator | null }) {
  const resource = useKnowledgeDetail(knowledgeId ?? null)
  const documents = resource.detail?.documents ?? []
  const selectedDocument = documents.find((item) => item.id === initialDocumentId) ?? documents.find((item) => !item.isFolder)
  useEffect(() => {
    if (!initialDocumentId && selectedDocument) {
      navigate('knowledge-detail', knowledgeId ?? undefined, { documentId: selectedDocument.id })
    }
  }, [initialDocumentId, knowledgeId, navigate, selectedDocument?.id])
  useEffect(() => {
    if (selectedDocument) void resource.openDocument(selectedDocument.id)
  }, [resource.openDocument, selectedDocument?.id])
  const body = resource.documentId === selectedDocument?.id ? contentText(resource.document) : ''
  const database = resource.detail?.database
  const [openingLocally, setOpeningLocally] = useState(false)
  const [previewRevision, setPreviewRevision] = useState(0)
  const downloadSelectedDocument = async () => {
    if (!selectedDocument) return
    const result = await resource.downloadDocument(selectedDocument.id, selectedDocument.name)
    if (result) toast.success(`已下载 ${result.filename}`, {
      className: 'fox-download-complete-toast',
      description: result.path,
      action: result.canOpenDirectly ? {
        label: '打开文件',
        onClick: () => void openDownloadedFile(result.path, false),
      } : {
        label: '打开所在文件夹',
        onClick: () => void openDownloadedFile(result.path, true),
      },
      cancel: result.canOpenDirectly ? {
        label: '打开所在文件夹',
        onClick: () => void openDownloadedFile(result.path, true),
      } : undefined,
      duration: 12_000,
    })
  }
  const openDownloadedFile = async (path: string, reveal: boolean) => {
    try {
      await desktopClient.openDownloadedFile(path, reveal)
    } catch (cause) {
      toast.error(reveal ? '无法打开所在文件夹' : '无法打开文件', {
        description: cause instanceof Error ? cause.message : String(cause),
      })
    }
  }
  const openSelectedDocumentLocally = async () => {
    if (!selectedDocument || openingLocally) return
    setOpeningLocally(true)
    let source: Awaited<ReturnType<typeof openKnowledgeCachedPreviewSource>> | null = null
    try {
      source = await openKnowledgeCachedPreviewSource({
        knowledgeBaseId: knowledgeId ?? '',
        documentId: selectedDocument.id,
        filename: selectedDocument.name,
        maxBytes: MAX_LOCAL_OPEN_BYTES,
      })
      await desktopClient.openKnowledgePreviewCache(source.cacheKey)
      toast.success(`已使用本机应用打开 ${selectedDocument.name}`)
    } catch (cause) {
      toast.error('无法使用本机应用打开文件', {
        description: cause instanceof Error ? cause.message : String(cause),
      })
    } finally {
      await source?.close()
      setOpeningLocally(false)
    }
  }
  const reloadSelectedDocumentPreview = () => {
    if (!selectedDocument) return
    setPreviewRevision((value) => value + 1)
    void resource.openDocument(selectedDocument.id)
    toast.success('正在重新加载预览', { description: selectedDocument.name })
  }
  const copySelectedDocumentInfo = async () => {
    if (!selectedDocument) return
    const text = formatKnowledgeDocumentInfo({
      knowledgeBase: database?.name ?? knowledgeId ?? '未知',
      filename: selectedDocument.name,
      kind: documentKind(selectedDocument),
      size: formatSize(selectedDocument.size),
      updatedAt: formatDate(selectedDocument.updatedAt),
      status: selectedDocument.status ?? '未知',
      documentId: selectedDocument.id,
    })
    try {
      if (!navigator.clipboard) throw new Error('当前环境不支持剪贴板')
      await navigator.clipboard.writeText(text)
      toast.success('已复制文件信息')
    } catch (cause) {
      toast.error('无法复制文件信息', {
        description: cause instanceof Error ? cause.message : String(cause),
      })
    }
  }
  return (
    <WorkspacePage
      className="fox-knowledge-detail-page"
      title={selectedDocument?.name ?? database?.name ?? '知识库'}
      sidebarCollapsed={sidebarCollapsed}
      onSidebar={onSidebar}
      onBack={() => navigate('knowledge')}
      actions={<>
        <div className="fox-knowledge-document-heading">
          <strong title={selectedDocument?.name ?? database?.name ?? '知识库'}>{selectedDocument?.name ?? database?.name ?? '知识库'}</strong>
          {selectedDocument && <DropdownMenu><DropdownMenuTrigger asChild><Button className="fox-document-title-more" variant="ghost" size="icon" aria-label="更多文件操作"><MoreHorizontal /></Button></DropdownMenuTrigger><DropdownMenuContent align="start" className="fox-document-actions-menu"><DropdownMenuItem onSelect={reloadSelectedDocumentPreview}><RotateCcw />重新加载预览</DropdownMenuItem><DropdownMenuSeparator /><DropdownMenuItem onSelect={() => void copySelectedDocumentInfo()}><Copy />复制文件信息</DropdownMenuItem></DropdownMenuContent></DropdownMenu>}
        </div>
        <Button size="sm" disabled={!database} onClick={() => database && onAskKnowledge(database)}><MessageSquare size={14} />使用知识助手提问</Button>
        {selectedDocument && <div className="fox-document-action-buttons">
          <Button variant="outline" size="sm" disabled={openingLocally} onClick={() => void openSelectedDocumentLocally()}>{openingLocally ? <LoaderCircle className="animate-spin" /> : <MonitorUp />}本机打开</Button>
          {resource.downloadingId === selectedDocument.id ? <><span className="fox-document-download-progress">{formatDownloadProgress(resource.downloadProgress)}</span><Button variant="outline" size="sm" onClick={() => void resource.cancelDownload()}><X />取消</Button></> : <Button variant="outline" size="sm" onClick={() => void downloadSelectedDocument()}><Download />下载原文件</Button>}
        </div>}
      </>}
    >
      {resource.error && <p className="fox-page-error">{resource.error}</p>}
      <div className="fox-library-layout is-document-only">
        <main className="fox-document-view">
          {selectedDocument ? <article className="fox-document-detail"><div>
            <div className="fox-document-info-card">
              <div className="fox-document-meta-strip is-inline">
                <span><small>类型</small><b title={documentKind(selectedDocument)}>{documentKind(selectedDocument)}</b></span>
                <span><small>大小</small><b>{formatSize(selectedDocument.size)}</b></span>
                <span><small>更新时间</small><b>{formatDate(selectedDocument.updatedAt)}</b></span>
                <span><small>处理状态</small><b title={selectedDocument.status ?? '未知'}>{selectedDocument.status ?? '未知'}</b></span>
              </div>
            </div>
            <KnowledgeFileViewer key={`${selectedDocument.id}:${previewRevision}`} knowledgeBaseId={knowledgeId ?? ''} document={selectedDocument} parsedContent={body} parsedContentLoading={resource.documentId === selectedDocument.id && resource.documentLoading} sourceLocator={sourceLocator} />
          </div></article> : <div className="fox-document-placeholder"><FolderOpen /><b>选择文件查看详情</b><p>{database?.description || '浏览知识库中已处理的文档内容。'}</p></div>}
        </main>
      </div>
    </WorkspacePage>
  )
}

export function KnowledgeGraphPage({ sidebarCollapsed, onSidebar, navigate, knowledgeId }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace; knowledgeId?: string | null }) {
  const resource = useKnowledgeDetail(knowledgeId ?? null)
  const canvasRef = useRef<KnowledgeGraphCanvasHandle>(null)
  const [query, setQuery] = useState('')
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [selectedType, setSelectedType] = useState('全部')
  const [depth, setDepth] = useState(2)
  const [maxNodes, setMaxNodes] = useState(100)
  const [zoom, setZoom] = useState(1)
  useEffect(() => { void resource.loadGraph(); void resource.loadGraphMetadata() }, [resource.loadGraph, resource.loadGraphMetadata])
  const rawGraph = useMemo(() => normalizeGraph(resource.graph), [resource.graph])
  const labels = useMemo(() => normalizeGraphLabels(resource.graphLabels, rawGraph.nodes), [rawGraph.nodes, resource.graphLabels])
  const graph = useMemo(() => filterGraph(rawGraph, selectedType), [rawGraph, selectedType])
  const selected = graph.nodes.find((node) => node.id === selectedId) ?? null
  const sourceDocumentId = selected ? graphSourceDocumentId(selected, graph.edges) : null
  const stats = normalizeGraphStats(resource.graphStats, rawGraph)
  const selectedRelations = selected ? graph.edges.filter((edge) => edge.source === selected.id || edge.target === selected.id) : []
  const load = (keyword = query) => {
    setSelectedId(null)
    void resource.loadGraph(keyword, depth, maxNodes)
    void resource.loadGraphMetadata()
  }
  return (
    <WorkspacePage className="fox-knowledge-graph-page" title="知识图谱" subtitle={`${resource.detail?.database.name ?? '知识图谱'} · 探索实体与文档之间的关系`} sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} onBack={() => navigate('knowledge-detail', knowledgeId ?? undefined)} actions={<Button variant="outline" size="sm" disabled={resource.graphLoading} onClick={() => load()}><RotateCcw size={14} />{resource.graphLoading ? '加载中' : '刷新图谱'}</Button>}>
      <div className="fox-graph-layout">
        <div className="fox-graph-toolbar">
          <label><Search size={14} /><Input aria-label="搜索节点" placeholder="搜索实体、概念或文档" value={query} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') load() }} /></label>
          <Select value={selectedType} onValueChange={(value) => { setSelectedType(value); setSelectedId(null) }}><SelectTrigger aria-label="实体类型"><SelectValue /></SelectTrigger><SelectContent>{['全部', ...labels].map((label) => <SelectItem key={label} value={label}>{label}</SelectItem>)}</SelectContent></Select>
          <Select value={String(depth)} onValueChange={(value) => setDepth(Number(value))}><SelectTrigger aria-label="查询深度"><SelectValue /></SelectTrigger><SelectContent>{[1, 2, 3].map((value) => <SelectItem key={value} value={String(value)}>深度 {value}</SelectItem>)}</SelectContent></Select>
          <Select value={String(maxNodes)} onValueChange={(value) => setMaxNodes(Number(value))}><SelectTrigger aria-label="节点上限"><SelectValue /></SelectTrigger><SelectContent>{[50, 100, 200].map((value) => <SelectItem key={value} value={String(value)}>最多 {value} 个节点</SelectItem>)}</SelectContent></Select>
          <Button size="sm" onClick={() => load()} disabled={resource.graphLoading}><Search />查询</Button>
          <div className="fox-graph-toolbar-stats"><span><b>{graph.nodes.length}</b> 节点</span><span><b>{graph.edges.length}</b> 关系</span></div>
        </div>
        <div className="fox-graph-workspace">
        <main className="fox-graph-canvas">
          <div className="fox-graph-canvas-head"><span><Sparkles />关系画布</span><small>{selected ? `已选择 ${selected.label}` : '单击查看详情，双击展开邻居'}</small></div>
          <div className="fox-graph-tools"><Button size="icon" variant="secondary" title="缩小" onClick={() => void canvasRef.current?.zoomBy(-.12)}><Minus /></Button><span>{Math.round(zoom * 100)}%</span><Button size="icon" variant="secondary" title="放大" onClick={() => void canvasRef.current?.zoomBy(.12)}><Plus /></Button><Button size="icon" variant="secondary" title="适应画布" onClick={() => void canvasRef.current?.fitView()}><Maximize2 /></Button></div>
          <KnowledgeGraphCanvas ref={canvasRef} nodes={graph.nodes} edges={graph.edges} selectedId={selectedId} onSelect={setSelectedId} onExpand={(node) => { setQuery(node.label); load(node.label) }} onZoomChange={setZoom} />
          {graph.nodes.length === 0 && <p className="fox-page-empty fox-graph-empty-state">{resource.graphLoading ? '正在加载知识图谱...' : '当前知识库暂无可浏览图谱，或图谱服务尚未完成构建。'}</p>}
        </main>
        <aside className="fox-graph-inspector">
          <div><span className="fox-document-icon">{selected?.type === 'Chunk' ? <FileText /> : <Bot />}</span><p><small>{selected ? selected.type : '知识图谱'}</small><b>{selected?.label ?? resource.detail?.database.name}</b></p></div>
          <p>{selected ? '查看节点属性、关联关系与来源文档。' : resource.detail?.database.description || '从画布选择一个节点，开始探索知识之间的联系。'}</p>
          {selected ? <><dl>{Object.entries(selected.properties).slice(0, 8).map(([key, value]) => <div key={key}><dt>{key}</dt><dd title={String(value)}>{formatGraphProperty(value)}</dd></div>)}</dl><section className="fox-graph-relations"><header><b>关联关系</b><span>{selectedRelations.length}</span></header>{selectedRelations.slice(0, 6).map((edge) => { const neighborId = edge.source === selected.id ? edge.target : edge.source; const neighbor = graph.nodes.find((node) => node.id === neighborId); return <button key={edge.id} onClick={() => setSelectedId(neighborId)}><Link2 /><span><b>{edge.label || '关联'}</b><small>{neighbor?.label ?? neighborId}</small></span><ChevronRight /></button> })}{selectedRelations.length === 0 && <small>当前子图中暂无关系</small>}</section><div className="fox-graph-inspector-actions"><Button size="sm" variant="outline" onClick={() => { setQuery(selected.label); load(selected.label) }}><Network />展开邻居</Button>{sourceDocumentId && <Button size="sm" onClick={() => navigate('knowledge-detail', knowledgeId ?? undefined, { documentId: sourceDocumentId })}><FileText />查看来源</Button>}</div></> : <><div className="fox-graph-overview"><span><Network /><b>{stats.nodes}</b><small>全部节点</small></span><span><Link2 /><b>{stats.edges}</b><small>全部关系</small></span><span><ListTree /><b>{labels.length}</b><small>实体类型</small></span></div><section className="fox-graph-guide"><b>如何探索</b><p>搜索关键词加载相关子图，单击节点查看详情，双击节点继续展开上下游关系。</p></section></>}
        </aside>
        </div>
      </div>
    </WorkspacePage>
  )
}

function formatSize(bytes: number) { if (!bytes) return '0 B'; if (bytes < 1024) return `${bytes} B`; if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`; return `${(bytes / 1024 / 1024).toFixed(1)} MB` }
function formatDate(value: string | null) { if (!value) return '未知时间'; const date = new Date(value); return Number.isNaN(date.getTime()) ? value : date.toLocaleDateString() }
function documentKind(document: KnowledgeDocumentRecord) { if (document.isFolder) return '文件夹'; const extension = document.name.split('.').pop()?.toLowerCase(); return ({ pdf: 'PDF 文档', doc: 'Word 文档', docx: 'Word 文档', xls: 'Excel 表格', xlsx: 'Excel 表格', csv: 'CSV 数据', md: 'Markdown', txt: '文本文件', json: 'JSON 数据', png: '图片', jpg: '图片', jpeg: '图片', webp: '图片' } as Record<string, string>)[extension ?? ''] ?? '文档' }
interface GraphNodeRecord { id: string; label: string; type: string; properties: Record<string, unknown> }
interface GraphEdgeRecord { id: string; source: string; target: string; label: string; properties: Record<string, unknown> }
function graphPayload(value: unknown): Record<string, unknown> {
  const root = value && typeof value === 'object' ? value as Record<string, unknown> : {}
  return root.data && typeof root.data === 'object' && !Array.isArray(root.data)
    ? root.data as Record<string, unknown>
    : root
}
function normalizeGraph(value: unknown): { nodes: GraphNodeRecord[]; edges: GraphEdgeRecord[] } {
  const data = graphPayload(value)
  const rawNodes = Array.isArray(data.nodes) ? data.nodes : Array.isArray(data.entities) ? data.entities : []
  const rawEdges = Array.isArray(data.edges) ? data.edges : Array.isArray(data.relationships) ? data.relationships : []
  const nodes = rawNodes.map((item, index) => { const node = item as Record<string, unknown>; const properties = node.properties && typeof node.properties === 'object' ? node.properties as Record<string, unknown> : {}; return { id: String(node.id ?? node._id ?? index), label: String(node.name ?? node.label ?? node.title ?? properties.name ?? properties.content_preview ?? `节点 ${index + 1}`), type: String(node.type ?? properties.label ?? 'Entity'), properties } })
  const edges = rawEdges.map((item, index) => { const edge = item as Record<string, unknown>; return { id: String(edge.id ?? `${edge.source_id ?? edge.source}-${edge.target_id ?? edge.target}-${index}`), source: String(edge.source_id ?? edge.source ?? ''), target: String(edge.target_id ?? edge.target ?? ''), label: String(edge.type ?? edge.label ?? ''), properties: edge.properties && typeof edge.properties === 'object' ? edge.properties as Record<string, unknown> : {} } }).filter((edge) => edge.source && edge.target)
  return { nodes, edges }
}
function normalizeGraphLabels(value: unknown, nodes: GraphNodeRecord[]) { const root = graphPayload(value); const labels = Array.isArray(root.labels) ? root.labels.map(String) : []; return [...new Set(labels.length ? labels : nodes.map((node) => node.type))].filter((label) => label && label !== 'MilvusKB') }
function normalizeGraphStats(value: unknown, graph: { nodes: GraphNodeRecord[]; edges: GraphEdgeRecord[] }) { const root = graphPayload(value); const stats = root.stats && typeof root.stats === 'object' ? root.stats as Record<string, unknown> : root; return { nodes: Number(stats.total_nodes ?? stats.node_count ?? graph.nodes.length), edges: Number(stats.total_edges ?? stats.edge_count ?? graph.edges.length) } }
function filterGraph(graph: { nodes: GraphNodeRecord[]; edges: GraphEdgeRecord[] }, type: string) { if (type === '全部') return graph; const nodes = graph.nodes.filter((node) => node.type === type); const ids = new Set(nodes.map((node) => node.id)); return { nodes, edges: graph.edges.filter((edge) => ids.has(edge.source) && ids.has(edge.target)) } }
function graphSourceDocumentId(node: GraphNodeRecord, edges: GraphEdgeRecord[]) { const direct = node.properties.file_id ?? node.properties.document_id ?? node.properties.fileId ?? node.properties.documentId; if (direct) return String(direct); const related = edges.find((edge) => (edge.source === node.id || edge.target === node.id) && (edge.properties.file_id ?? edge.properties.document_id)); return related ? String(related.properties.file_id ?? related.properties.document_id) : null }
function formatGraphProperty(value: unknown) { if (Array.isArray(value)) return value.join(', '); if (value && typeof value === 'object') return JSON.stringify(value); return String(value ?? '') }
function formatDownloadProgress(value: { bytesWritten: number; totalBytes: number | null } | null) { if (!value) return '准备下载'; if (value.totalBytes && value.totalBytes > 0) return `${Math.min(100, Math.round(value.bytesWritten / value.totalBytes * 100))}%`; return `${formatSize(value.bytesWritten)} 已下载` }
