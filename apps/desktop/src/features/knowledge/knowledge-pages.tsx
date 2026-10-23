import { Fragment, useEffect, useId, useMemo, useRef, useState } from 'react'
import { Bot, ChevronRight, Copy, Download, FileText, FolderOpen, Link2, ListTree, LoaderCircle, LogIn, Maximize2, MessageSquare, Minus, MonitorUp, MoreHorizontal, Network, Plus, RefreshCw, RotateCcw, Search, Server, Sparkles, X } from 'lucide-react'
import { notify as toast } from '@/features/notifications'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { ExpandingSearch } from '@/components/arc/expanding-search/expanding-search'
import { useLibraryBrowser, navigateLibraryFolder, setLibraryView, publishLibraryFolders } from '@/features/local-knowledge/library-browser-state'
import { Button } from '@/components/ui/button'
import { FoxEmptyState } from '@/components/fox-ui/empty-state'
import { SkeletonContent } from '@/components/ui/skeleton'
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
import { KnowledgeGraphCanvas, type KnowledgeGraphCanvasHandle, type KnowledgeGraphNodeShape } from './knowledge-graph-canvas'
import { YUXI_LOGIN_RETURN_KEY, YUXI_SERVICE_RETURN_KEY } from './knowledge-navigation'
import { LibraryFileManager, LibraryViewSwitch } from '@/features/local-knowledge/library-file-manager'
import { remoteLibraryFiles, type LibraryFile } from '@/features/local-knowledge/library-file-model'
import { ArrowLeft } from 'lucide-react'

const MAX_LOCAL_OPEN_BYTES = 500 * 1024 * 1024

function asCard(item: KnowledgeBaseRecord, index: number): KnowledgeCardData {
  const progress = item.fileCount > 0 ? Math.round(item.processedCount / item.fileCount * 100) : item.status === 'active' ? 100 : 0
  return { ...item, types: item.kbType || '资料库', progress, agents: 0, recent: item.updatedAt ? new Date(item.updatedAt).toLocaleDateString() : '来自资料库服务', tone: ['blue', 'violet', 'green', 'amber', 'gray'][index % 5] }
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
  const typeOptions = Array.from(new Set(allCards.map((item) => item.kbType || '资料库')))
  const cards = allCards
    .filter((item) => !normalizedQuery || [item.name, item.description, item.kbType]
      .some((value) => value?.toLocaleLowerCase().includes(normalizedQuery)))
    .filter((item) => statusFilter === 'all' || (statusFilter === 'ready' ? item.progress >= 100 : statusFilter === 'processing' ? item.progress > 0 && item.progress < 100 : item.progress === 0))
    .filter((item) => typeFilter === 'all' || (item.kbType || '资料库') === typeFilter)
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
    <WorkspacePage title="远程资料库" subtitle="浏览你有权限访问的资料" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
      <section className="fox-page-content fox-knowledge-list-page">
        <div className="fox-page-intro fox-list-page-intro"><h1>远程资料库</h1><label className="fox-list-page-search"><Search size={14} /><Input aria-label="搜索资料库" placeholder="搜索资料库" value={query} onChange={(event) => setQuery(event.target.value)} /></label></div>
        <div className="fox-page-toolbar fox-agent-toolbar fox-knowledge-toolbar">
          <Select value={statusFilter} onValueChange={setStatusFilter}><SelectTrigger className="fox-agent-filter"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">全部状态</SelectItem><SelectItem value="ready">可使用</SelectItem><SelectItem value="processing">处理中</SelectItem><SelectItem value="pending">待处理</SelectItem></SelectContent></Select>
          <Select value={typeFilter} onValueChange={setTypeFilter}><SelectTrigger className="fox-agent-filter"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">全部类型</SelectItem>{typeOptions.map((type) => <SelectItem key={type} value={type}>{type}</SelectItem>)}</SelectContent></Select>
          <Select value={contentFilter} onValueChange={setContentFilter}><SelectTrigger className="fox-agent-filter"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">全部内容</SelectItem><SelectItem value="populated">包含文件</SelectItem><SelectItem value="empty">空资料库</SelectItem></SelectContent></Select>
          <span className="fox-agent-result-count">{cards.length} 个结果</span>
          <Button variant="outline" size="sm" className="fox-agent-refresh" title="刷新" aria-label="刷新" onClick={() => void resource.refresh()}><RefreshCw /></Button>
        </div>
        {(yuxi.loading || yuxiUser.loading || (yuxiUser.user && resource.loading && resource.items.length === 0)) && <SkeletonContent label="正在读取远程资料库…" lines={4} avatar />}
        {resource.error && yuxiUser.user && <FoxEmptyState variant="error" title="远程资料库加载失败" description={resource.error} action={<Button variant="outline" size="sm" onClick={() => void resource.refresh()}>重试</Button>} />}
        {!yuxi.loading && !yuxi.service && <FoxEmptyState icon={<Server />} title="先连接资料库服务，再登录账户" description="先连接资料库服务，再登录账户访问资料。" action={<Button onClick={openService}>配置服务</Button>} />}
        {!yuxi.loading && yuxi.service && !yuxiUser.loading && !yuxiUser.user && <FoxEmptyState variant={yuxiUser.error ? 'error' : 'empty'} icon={<LogIn />} title={connected ? '资料库服务已连接，账户尚未登录' : '需要登录资料库'} description={yuxiUser.error ?? '浏览器中的登录态不会与 Fox 自动共享，请在 Fox 中登录一次。'} action={<Button onClick={openLogin}>登录账户</Button>} />}
        {!resource.loading && !resource.error && yuxiUser.user && resource.items.length === 0 && <FoxEmptyState title="当前账户没有可查看的资料库" description="请确认账户权限，或在资料库服务中添加资料。" />}
        {!resource.loading && !resource.error && yuxiUser.user && resource.items.length > 0 && cards.length === 0 && <FoxEmptyState variant="search" title="没有符合当前筛选条件的资料库" description={query.trim() ? `没有匹配“${query.trim()}”的资料库。` : '调整状态、类型或内容筛选后再试。'} action={<Button variant="outline" size="sm" onClick={() => { setQuery(''); setStatusFilter('all'); setTypeFilter('all'); setContentFilter('all') }}>清空筛选</Button>} />}
        <div className="fox-entity-grid fox-shadcn-entity-grid fox-knowledge-grid">{cards.map((knowledge) => <KnowledgeCard key={knowledge.id} knowledge={knowledge} onOpen={() => navigate('knowledge-detail', knowledge.id)} />)}</div>
      </section>
    </WorkspacePage>
  )
}

export function KnowledgeDetailPage({ sidebarCollapsed, onSidebar, navigate, onAskKnowledge, knowledgeId, initialDocumentId, sourceLocator }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace; onAskKnowledge: (knowledgeBase: KnowledgeBaseRecord) => void; knowledgeId?: string | null; initialDocumentId?: string | null; sourceLocator?: KnowledgeSourceLocator | null }) {
  const resource = useKnowledgeDetail(knowledgeId ?? null)
  const documents = resource.detail?.documents ?? []
  const selectedDocument = documents.find((item) => !item.isFolder && item.id === initialDocumentId)
  const managedFiles = useMemo(() => remoteLibraryFiles(documents), [documents])
  const browserKey = `remote:${knowledgeId ?? ''}`
  const browser = useLibraryBrowser(browserKey)
  const [fileQuery, setFileQuery] = useState('')
  useEffect(() => { if (resource.detail) publishLibraryFolders(browserKey, managedFiles) }, [browserKey, managedFiles, resource.detail])
  const [downloadingBatch, setDownloadingBatch] = useState(false)
  const [batchDownloadMessage, setBatchDownloadMessage] = useState<string | null>(null)
  const batchCancelled = useRef(false)
  const previewReturnFocus = useRef<HTMLElement | null>(null)
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
      description: result.path,
      sourceType: 'knowledge_download',
      sourceId: selectedDocument.id,
      localFile: { path: result.path, canOpenDirectly: result.canOpenDirectly },
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
  const downloadFiles = async (files: LibraryFile[]) => {
    if (downloadingBatch) return
    setDownloadingBatch(true)
    setBatchDownloadMessage(null)
    batchCancelled.current = false
    const unfinished: string[] = []
    try {
      for (const file of files) {
        if (batchCancelled.current) { unfinished.push(file.name); continue }
        try {
          const result = await resource.downloadDocument(file.id, file.name)
          if (result) toast.success(`已下载 ${result.filename}`, { description: result.path, sourceType: 'knowledge_download', sourceId: file.id, localFile: { path: result.path, canOpenDirectly: result.canOpenDirectly } })
          else unfinished.push(file.name)
        } catch { unfinished.push(file.name) }
      }
      if (unfinished.length) setBatchDownloadMessage(`${batchCancelled.current ? '已停止下载。' : ''}以下文件未完成下载：${unfinished.join('、')}。可以重新选择这些文件后重试。`)
    } finally { setDownloadingBatch(false) }
  }
  return <WorkspacePage className="fox-knowledge-detail-page" title={database?.name ?? '远程资料库'} sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} onBack={() => navigate('local-knowledge')}>
    <header className="fox-library-remote-heading fox-library-detail-topbar">
      <Button variant="ghost" size="icon" aria-label="返回资料库" onClick={() => navigate('local-knowledge')}><ArrowLeft /></Button>
      <nav className="fox-library-breadcrumb" aria-label="资料库路径"><button type="button" aria-current={!browser.path ? 'page' : undefined} onClick={() => navigateLibraryFolder(browserKey, '')}>{database?.name ?? '远程资料库'}</button>{browser.path.split('/').filter(Boolean).map((name, index, parts) => <Fragment key={parts.slice(0, index + 1).join('/')}><span aria-hidden="true">/</span><button type="button" aria-current={index === parts.length - 1 ? 'page' : undefined} onClick={() => navigateLibraryFolder(browserKey, parts.slice(0, index + 1).join('/'))}>{name}</button></Fragment>)}</nav>
      <div className="fox-kb-search-control-slot"><ExpandingSearch reserveSpace clearLabel="清空搜索" className="fox-kb-expanding-search" anchor="end" label="搜索远程资料库文件" placeholder="搜索文件名" items={[]} value={fileQuery} onValueChange={setFileQuery} onSearch={setFileQuery} showResults={false} expandedWidth={280} /></div>
      <LibraryViewSwitch view={browser.view} onChange={(view) => setLibraryView(browserKey, view)} />
      <Button variant="ghost" size="icon" aria-label="刷新远程资料库" disabled={resource.loading} onClick={() => void resource.refresh()}><RefreshCw /></Button><Button variant="outline" size="sm" disabled={!database} onClick={() => navigate('knowledge-graph', knowledgeId ?? undefined)}><Network />关系图</Button><Button size="sm" disabled={!database} onClick={() => database && onAskKnowledge(database)}><MessageSquare />就此资料提问</Button>
    </header>
    <div className="fox-library-remote-content">{resource.loading && !database && <SkeletonContent label="正在读取远程资料库" lines={6} />}{resource.error && <FoxEmptyState variant="error" title="远程资料加载失败" description={resource.error} action={<Button variant="outline" onClick={() => void resource.refresh()}>重试</Button>} />}{batchDownloadMessage && <p role="alert" className="fox-library-download-message">{batchDownloadMessage}</p>}{downloadingBatch && <div className="fox-library-download-message" role="status">正在下载所选文件 <Button variant="outline" size="sm" onClick={() => { batchCancelled.current = true; void resource.cancelDownload() }}>停止下载</Button></div>}{database && <LibraryFileManager contentOnly view={browser.view} path={browser.path} onPathChange={(path) => navigateLibraryFolder(browserKey, path)} searchQuery={fileQuery} onSearchQueryChange={setFileQuery} files={managedFiles} busy={downloadingBatch} onOpen={(file) => navigate('knowledge-detail', knowledgeId ?? undefined, { documentId: file.id })} onDownload={(files) => void downloadFiles(files)} />}</div>
    <Dialog open={Boolean(initialDocumentId)} onOpenChange={(open) => { if (!open) navigate('knowledge-detail', knowledgeId ?? undefined) }}><DialogContent className="fox-library-preview-dialog fox-library-remote-preview" aria-describedby={undefined} onOpenAutoFocus={() => { previewReturnFocus.current = globalThis.document.activeElement as HTMLElement }} onCloseAutoFocus={(event) => { event.preventDefault(); if (previewReturnFocus.current?.isConnected) previewReturnFocus.current.focus() }}>
      <header className="fox-file-preview-toolbar">
        <DialogTitle className="fox-file-preview-title" title={selectedDocument?.name}>{selectedDocument?.name ?? '文件预览'}</DialogTitle>
        {selectedDocument && <div className="fox-file-preview-meta" title={`${documentKind(selectedDocument)} · ${formatSize(selectedDocument.size)} · ${formatDate(selectedDocument.updatedAt)} · ${selectedDocument.status ?? '未知'}`}>{documentKind(selectedDocument)} · {formatSize(selectedDocument.size)} · {formatDate(selectedDocument.updatedAt)} · {selectedDocument.status ?? '未知'}</div>}
        {selectedDocument && <div className="fox-file-preview-actions">
          <Button variant="ghost" size="icon" aria-label="就此资料提问" title="就此资料提问" disabled={!database} onClick={() => database && onAskKnowledge(database)}><MessageSquare /></Button>
          <Button variant="ghost" size="icon" title="刷新预览" aria-label="刷新预览" onClick={reloadSelectedDocumentPreview}><RotateCcw /></Button>
          <Button variant="ghost" size="icon" aria-label="复制文件信息" title="复制文件信息" onClick={() => void copySelectedDocumentInfo()}><Copy /></Button>
          <Button variant="ghost" size="icon" aria-label="本机打开" title="本机打开" disabled={openingLocally} onClick={() => void openSelectedDocumentLocally()}><MonitorUp /></Button>
          {resource.downloadingId === selectedDocument.id ? <><span className="fox-file-preview-progress" role="status">{formatDownloadProgress(resource.downloadProgress)}</span><Button variant="ghost" size="icon" aria-label="取消下载" title="取消下载" onClick={() => void resource.cancelDownload()}><X /></Button></> : <Button variant="ghost" size="icon" aria-label="下载原件" title="下载原件" onClick={() => void downloadSelectedDocument()}><Download /></Button>}
        </div>}
      </header>
      {resource.error && <FoxEmptyState variant="error" title="资料加载失败" description={resource.error} action={<Button variant="outline" size="sm" onClick={() => void (selectedDocument ? resource.openDocument(selectedDocument.id) : resource.refresh())}>重试</Button>} />}
      <div className="fox-library-modal-body">{selectedDocument ? <KnowledgeFileViewer key={`${selectedDocument.id}:${previewRevision}`} knowledgeBaseId={knowledgeId ?? ''} document={selectedDocument} parsedContent={body} parsedContentLoading={resource.documentId === selectedDocument.id && resource.documentLoading} sourceLocator={sourceLocator} /> : resource.loading ? <SkeletonContent label="正在读取文件" lines={5} /> : <FoxEmptyState title="未找到文件" description="文件可能已被移除，请刷新资料库。" />}</div>
    </DialogContent></Dialog>
  </WorkspacePage>
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
  const [nodeShape, setNodeShape] = useState<KnowledgeGraphNodeShape>('rect')
  const [inspectorOpen, setInspectorOpen] = useState(true)
  const inspectorId = useId()
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
    <WorkspacePage className="fox-knowledge-graph-page" title="关系图" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
      <div className="fox-graph-layout">
        <header className="fox-graph-toolbar">
          <Button variant="ghost" size="icon" aria-label="返回资料库" title="返回资料库" onClick={() => navigate('knowledge-detail', knowledgeId ?? undefined)}><ArrowLeft /></Button>
          <h1>关系图</h1>
          <label><Search size={14} /><Input aria-label="搜索条目" placeholder="搜索实体、概念或文档" value={query} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter' && !event.nativeEvent.isComposing) load() }} /></label>
          <Select value={selectedType} onValueChange={(value) => { setSelectedType(value); setSelectedId(null) }}><SelectTrigger aria-label="类别"><SelectValue /></SelectTrigger><SelectContent>{['全部', ...labels].map((label) => <SelectItem key={label} value={label}>{label}</SelectItem>)}</SelectContent></Select>
          <Select value={String(depth)} onValueChange={(value) => setDepth(Number(value))}><SelectTrigger aria-label="查询深度"><SelectValue /></SelectTrigger><SelectContent>{[1, 2, 3].map((value) => <SelectItem key={value} value={String(value)}>深度 {value}</SelectItem>)}</SelectContent></Select>
          <Select value={String(maxNodes)} onValueChange={(value) => setMaxNodes(Number(value))}><SelectTrigger aria-label="节点显示上限" title="本次查询最多显示的节点数"><SelectValue /></SelectTrigger><SelectContent>{[50, 100, 200].map((value) => <SelectItem key={value} value={String(value)}>节点 {value}</SelectItem>)}</SelectContent></Select>
          <Button size="sm" onClick={() => load()} disabled={resource.graphLoading}><Search />查询</Button>
          <Button variant="ghost" size="icon" aria-label={resource.graphLoading ? '正在刷新图谱' : '刷新图谱'} title="刷新图谱" disabled={resource.graphLoading} onClick={() => load()}><RotateCcw size={14} /></Button>
          <Select value={nodeShape} onValueChange={(value) => setNodeShape(value as KnowledgeGraphNodeShape)}><SelectTrigger aria-label="节点形状"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="rect">矩形节点</SelectItem><SelectItem value="circle">圆形节点</SelectItem></SelectContent></Select>
          <Button size="sm" variant="outline" aria-expanded={inspectorOpen} aria-controls={inspectorId} onClick={() => setInspectorOpen((open) => !open)}>{inspectorOpen ? '收起详情' : '显示详情'}</Button>
        </header>
        <div className={`fox-graph-workspace ${inspectorOpen ? '' : 'is-inspector-collapsed'}`}>
        <main className="fox-graph-canvas">
          <div className="fox-graph-canvas-head"><div className="fox-graph-canvas-heading"><span><Sparkles />关系画布</span><div className="fox-graph-toolbar-stats" aria-label="当前画布统计"><span><b>{graph.nodes.length}</b> 条目</span><span><b>{graph.edges.length}</b> 关系</span></div></div><small>{selected ? `已选择 ${selected.label}` : '单击查看详情，双击展开关联'}</small></div>
          <div className="fox-graph-tools"><Button size="icon" variant="secondary" title="缩小" onClick={() => void canvasRef.current?.zoomBy(-.12)}><Minus /></Button><span>{Math.round(zoom * 100)}%</span><Button size="icon" variant="secondary" title="放大" onClick={() => void canvasRef.current?.zoomBy(.12)}><Plus /></Button><Button size="icon" variant="secondary" title="显示全图" onClick={() => void canvasRef.current?.fitView()}><Maximize2 /></Button></div>
          <KnowledgeGraphCanvas ref={canvasRef} nodes={graph.nodes} edges={graph.edges} selectedId={selectedId} nodeShape={nodeShape} onSelect={(id) => { setSelectedId(id); if (id) setInspectorOpen(true) }} onExpand={(node) => { setQuery(node.label); load(node.label) }} onZoomChange={setZoom} />
          {graph.nodes.length === 0 && (resource.graphLoading
            ? <SkeletonContent className="fox-graph-empty-state" label="正在加载关系图…" lines={4} />
            : <FoxEmptyState className="fox-graph-empty-state" variant={resource.error ? 'error' : query.trim() || selectedType !== '全部' ? 'search' : 'empty'} title={resource.error ? '关系图加载失败' : query.trim() || selectedType !== '全部' ? '没有匹配的关系图条目' : '资料库暂无可浏览图谱'} description={resource.error ?? '调整查询条件，或等待图谱服务完成构建。'} action={<Button variant="outline" size="sm" onClick={() => load()}>重新查询</Button>} />)}
        </main>
        <aside id={inspectorId} className="fox-graph-inspector" hidden={!inspectorOpen} aria-label="节点详情">
          <div><span className="fox-document-icon">{selected?.type === 'Chunk' ? <FileText /> : <Bot />}</span><p><small>{selected ? selected.type : '关系图'}</small><b>{selected?.label ?? resource.detail?.database.name}</b></p></div>
          <p>{selected ? '查看条目属性、关联关系与来源文档。' : resource.detail?.database.description || '从画布选择一个条目，开始探索资料之间的联系。'}</p>
          {selected ? <><dl>{Object.entries(selected.properties).slice(0, 8).map(([key, value]) => <div key={key}><dt>{key}</dt><dd title={String(value)}>{formatGraphProperty(value)}</dd></div>)}</dl><section className="fox-graph-relations"><header><b>关联关系</b><span>{selectedRelations.length}</span></header>{selectedRelations.slice(0, 6).map((edge) => { const neighborId = edge.source === selected.id ? edge.target : edge.source; const neighbor = graph.nodes.find((node) => node.id === neighborId); return <button key={edge.id} onClick={() => setSelectedId(neighborId)}><Link2 /><span><b>{edge.label || '关联'}</b><small>{neighbor?.label ?? neighborId}</small></span><ChevronRight /></button> })}{selectedRelations.length === 0 && <small>当前子图中暂无关系</small>}</section><div className="fox-graph-inspector-actions"><Button size="sm" variant="outline" onClick={() => { setQuery(selected.label); load(selected.label) }}><Network />展开关联</Button>{sourceDocumentId && <Button size="sm" onClick={() => navigate('knowledge-detail', knowledgeId ?? undefined, { documentId: sourceDocumentId })}><FileText />查看来源</Button>}</div></> : <><div className="fox-graph-overview"><span><Network /><b>{stats.nodes}</b><small>全部节点</small></span><span><Link2 /><b>{stats.edges}</b><small>全部关系</small></span><span><ListTree /><b>{labels.length}</b><small>类别</small></span></div><section className="fox-graph-guide"><b>如何探索</b><p>搜索关键词加载相关子图，单击节点查看详情，双击节点继续展开上下游关系。</p></section></>}
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
