// Browser fixture with synthetic gateway data. This is not a production route.
import { createRoot } from 'react-dom/client'
import { useState } from 'react'
import { TooltipProvider } from '../../src/components/ui/tooltip'
import '../../src/styles/globals.css'
import '../../src/styles/workbench.css'
import '../../src/styles/workspace-pages.css'
import '../../src/features/chat/shell/fox-sidebar.css'

;(globalThis as any).isTauri = true
const { desktopClient } = await import('../../src/features/conversations/api/desktop-client')
const { LocalKnowledgeWorkspace, LocalKnowledgeSidebarNavigation, createMockLocalKnowledgeGateway } = await import('../../src/features/local-knowledge')
const { KnowledgeDetailPage } = await import('../../src/features/knowledge/knowledge-pages')
const query = new URLSearchParams(location.search)
const remote = query.get('remote') ?? 'ready'
document.documentElement.dataset.colorTheme = query.get('theme') ?? 'graphite'
if (query.has('dark')) document.documentElement.classList.add('dark')
document.body.style.margin = '0'

const remoteBase = { id: 'fixture-remote', name: '产品文档', description: '团队共享的需求与规格说明', kbType: 'standard', status: 'active', fileCount: 5, processedCount: 3, rowCount: 0, createdAt: null, updatedAt: '2026-10-12T09:00:00Z' }
const remoteDocuments = [
  { id: 'remote-folder', name: '需求', parentId: null, isFolder: true, size: 0, status: null, updatedAt: null },
  ...['2026 Q4 需求池.docx', '客户访谈纪要.pdf', '搜索体验改版 PRD.md', '竞品功能对比.xlsx', '季度规划评审.pptx'].map((name, index) => ({ id: `remote-doc-${index}`, name, parentId: null, isFolder: false, size: 840000 + index * 18000, status: index === 4 ? 'failed' : index === 2 ? 'processing' : 'done', createdAt: null, updatedAt: '2026-10-12T09:00:00Z' })),
]
desktopClient.getYuxiService = async () => remote === 'off' ? null : ({ id: 'fixture-yuxi', name: 'Yuxi', baseUrl: 'https://yuxi.fixture.invalid', credentialConfigured: true, lastStatus: 'connected' } as any)
desktopClient.getYuxiUser = async () => { if (remote === 'error') throw new Error('测试夹具：服务器没有响应'); return { id: 'fixture-user', username: 'hury' } as any }
desktopClient.listKnowledgeBases = async () => [remoteBase]
desktopClient.getKnowledgeDetail = async () => ({ database: remoteBase, documents: remoteDocuments })
desktopClient.getKnowledgeDocument = async () => '# 测试夹具文档\n\n这段内容只用于核对阅读路由与原有文档 ID。'
desktopClient.listenKnowledgeDownloadProgress = async () => () => {}
desktopClient.downloadKnowledgeDocument = async (_downloadId, _baseId, documentId, filename) => { (window as any).__libraryFixture.downloads.push(documentId); if (documentId === 'remote-doc-4') throw new Error('测试夹具：下载失败'); return { filename, path: `D:/fixture/${filename}`, bytesWritten: 100, canOpenDirectly: false } as any }

const gateway = createMockLocalKnowledgeGateway()
const originalListDocuments = gateway.listDocuments
const originalReadRange = gateway.readDocumentFileRange
gateway.readDocumentFileRange = async (baseId, documentId, start, end) => documentId.startsWith('fixture-') ? new TextEncoder().encode('# 目录中的说明\n\n这是浏览器夹具内容，用于检查文档阅读与问答引用。').slice(start, end + 1) : originalReadRange(baseId, documentId, start, end)
const detailFiles = ['人工智能赋能自动化码头设备智能运维.docx', 'final-layout.pdf', '人工智能赋能自动化码头设备智能运维.md', '修订说明（逻辑修订）.md', '项目预算明细表.xlsx', '答辩材料.pptx', '申报书（盖章版）.pdf']
gateway.listDocuments = async (id, options) => id !== 'local-product-docs' ? originalListDocuments(id, options) : ({ total: detailFiles.length + 1, items: [...detailFiles.map((name, index) => ({ id: `fixture-document-${index}`, knowledgeBaseId: id, name, relativePath: name, extension: name.split('.').pop() ?? '', mimeType: '', sizeBytes: index === 1 ? 559000 : (index + 1) * 280000, status: index === 4 ? 'error' : index === 2 ? 'parsing' : 'indexed', chunkCount: 20, sourceRevision: null, parseError: index === 4 ? '测试夹具：解析失败' : null, importedAt: '2026-10-10T09:00:00Z', updatedAt: '2026-10-12T09:00:00Z' })), { id: 'fixture-nested', knowledgeBaseId: id, name: '目录中的说明.md', relativePath: '论文/目录中的说明.md', extension: 'md', mimeType: 'text/markdown', sizeBytes: 1000, status: 'indexed', chunkCount: 1, sourceRevision: null, parseError: null, importedAt: '', updatedAt: '' }] as any })
gateway.listFolders = async () => ['论文', '论文/青年业务论文', '附件', '预算', '图片素材'].map((relativePath) => ({ knowledgeBaseId: 'local-product-docs', name: relativePath.split('/').pop()!, relativePath, documentCount: relativePath === '论文' ? 1 : 0 }))
gateway.pickImportFiles = async () => [
  { name: '申报书（盖章版）.pdf', sourcePath: 'D:/材料/2026/申报书（盖章版）.pdf', mimeType: 'application/pdf', sizeBytes: 2400000 },
  { name: '答辩材料.pptx', sourcePath: 'E:/备份/答辩/答辩材料.pptx', mimeType: '', sizeBytes: 6200000 },
]
const operations: unknown[] = []
if (query.has('realReader')) {
  const assets = await Promise.all(['sample.pdf', 'sample.xlsx'].map(async (name) => ({ name, bytes: new Uint8Array(await (await fetch(`/tests/fixtures/preview-assets/${name}`)).arrayBuffer()) })))
  gateway.listDocuments = async (knowledgeBaseId) => ({ total: assets.length, items: assets.map(({ name, bytes }, index) => ({ id: `preview-${index}`, knowledgeBaseId, name, relativePath: name, extension: name.split('.').pop()!, mimeType: '', sizeBytes: bytes.length, status: 'indexed', chunkCount: 1, sourceRevision: null, parseError: null, importedAt: '', updatedAt: '' })) })
  gateway.readDocumentFileRange = async (_base, id, start, end) => assets[Number(id.split('-')[1])].bytes.slice(start, end + 1)
}
const extraFolders: any[] = []
const listFixtureFolders = gateway.listFolders
gateway.listFolders = async (id) => [...await listFixtureFolders(id), ...extraFolders.filter((folder) => folder.knowledgeBaseId === id)]
gateway.createFolder = async (knowledgeBaseId, name, parentPath = '') => { const folder = { knowledgeBaseId, name, relativePath: [parentPath, name].filter(Boolean).join('/'), documentCount: 0 }; extraFolders.push(folder); return folder }
const originalStartImport = gateway.startImport
gateway.startImport = async (request) => { operations.push({ action: 'import', request }); return originalStartImport(request) }
gateway.reparseDocument = async (baseId, documentId) => { operations.push({ action: 'reparse', baseId, documentId }); return { operationId: `fixture-reparse-${documentId}`, acceptedAt: new Date().toISOString() } }
gateway.deleteDocument = async (baseId, documentId) => { operations.push({ action: 'remove-copy', baseId, documentId }); return true }
if (query.has('folders')) {
  let sources = [{ id: 'fixture-folder', displayName: '项目资料', rootPath: 'D:/已整理/项目资料', fileCount: 125, totalSize: 125000, lastScannedAt: '', createdAt: '', updatedAt: '' }]
  const files = Array.from({ length: 125 }, (_, index) => ({ id: `folder-file-${index}`, sourceId: 'fixture-folder', sourceName: '项目资料', sourcePath: 'D:/已整理/项目资料', absolutePath: `D:/已整理/项目资料/方案/归档/资料-${index}.md`, relativePath: `方案/归档/资料-${index}.md`, name: `资料-${index}.md`, extension: 'md', mimeType: 'text/markdown', sizeBytes: 1000, modifiedAt: '2026-10-15T10:00:00Z' }))
  gateway.listFileSources = async () => [...sources]
  gateway.listLocalFiles = async (options = {}) => { operations.push({ action: 'folder-page', offset: options.offset }); return options.sourceId && options.sourceId !== 'fixture-folder' ? [] : files.slice(options.offset ?? 0, (options.offset ?? 0) + (options.limit ?? 100)) }
  gateway.pickFileSourceFolder = async () => query.has('cancelPicker') ? null : 'D:/新增资料'
  gateway.addFileSource = async (path) => { operations.push({ action: 'add-folder', path }); const source = { ...sources[0], id: 'added-folder', displayName: '新增资料', rootPath: path, fileCount: 0 }; if (!sources.some((item) => item.id === source.id)) sources.push(source); return source }
  gateway.updateFileSource = async (id, displayName, description) => { operations.push({ action: 'edit-folder', id, displayName, description }); sources = sources.map((source) => source.id === id ? { ...source, displayName, description } : source); return sources.find((source) => source.id === id)! }
  gateway.removeFileSource = async (id) => { operations.push({ action: 'remove-folder', id }); sources = sources.filter((source) => source.id !== id); return true }
  gateway.rescanFileSource = async (id) => { operations.push({ action: 'refresh-folder', id }); if (query.has('refreshError')) throw Error('文件夹暂时无法访问'); return sources.find((source) => source.id === id)! }
  gateway.readLocalFileRange = async () => new TextEncoder().encode('# 文件夹预览\n\n本地目录中的资料。')
  gateway.revealLocalFile = async (id) => { operations.push({ action: 'reveal-file', id }); return true }
  if (query.has('realPreview')) {
    const assets = await Promise.all(['sample.pdf', 'sample.xlsx'].map(async (name) => ({ name, bytes: new Uint8Array(await (await fetch(`/tests/fixtures/preview-assets/${name}`)).arrayBuffer()) })))
    files.splice(0, files.length, ...assets.map(({ name, bytes }, index) => ({ ...files[0], id: `preview-${index}`, name, relativePath: name, extension: name.split('.').pop()!, mimeType: '', sizeBytes: bytes.length })))
    gateway.readLocalFileRange = async (id, start, end) => assets[Number(id.split('-')[1])].bytes.slice(start, end + 1)
  }
}
if (query.has('tree')) {
  gateway.listFolders = async (id) => Array.from({length: 60}, (_, index) => ({knowledgeBaseId:id,name:`目录 ${index + 1}`,relativePath:`目录 ${index + 1}`,documentCount:0}))
}
if (query.has('slow')) {
  const delays: Record<string, number> = { listKnowledgeBases: 150, getKnowledgeBase: 300, listJobs: 1200, listEmbeddingModels: 2000, listDocuments: 150, listFolders: 150 }
  for (const [method, delay] of Object.entries(delays)) {
    const original = (gateway as any)[method].bind(gateway)
    ;(gateway as any)[method] = async (...args: any[]) => {
      operations.push({ action: method, at: performance.now() })
      await new Promise((resolve) => setTimeout(resolve, delay))
      return original(...args)
    }
  }
}
;(window as any).__libraryFixture = { operations, downloads: [], generation: crypto.randomUUID(), kind: 'synthetic browser fixture', gateway }

const { Sidebar } = await import('../../src/features/chat/shell/fox-sidebar')
const notices = Array.from({ length: 50 }, (_, i) => ({ id: `notice-${i}`, title: `任务完成 ${i+1}`, body: '通知滚动测试内容', kind: 'completed', status: 'completed', sourceType: 'task', action: { type: 'none' }, severity: 'info', readAt: null, updatedAt: new Date().toISOString(), createdAt: new Date().toISOString() }))
desktopClient.listAppNotifications = async () => notices as any
desktopClient.getNotificationPreferences = async () => ({ badge: true, sound: false }) as any
desktopClient.markAllAppNotificationsRead = async () => { notices.forEach((item: any) => item.readAt = new Date().toISOString()); return undefined as any }
desktopClient.getUserProfile = async () => ({ name: '测试用户', avatar: null }) as any
const noop = () => {}
function SidebarFixture() {
  return <div className="fox-app fox-shell" style={{ height: '100vh', display: 'flex' }}><Sidebar collapsed={false} assistantMode="assistant" activeView="chat" newChatActive={false} runtimeConversations={Array.from({ length: 65 }, (_, i) => ({ id: `conversation-${i}`, title: `测试对话 ${i+1}`, projectRoot: i < 30 ? `D:/project-${Math.floor(i/5)}` : undefined, createdAt: i, time: '现在' })) as any} onModeChange={noop} onNewChat={noop} onNewProjectChat={noop} onAddProject={noop} onOpenConversation={noop} onRenameConversation={noop} onPinConversation={noop} onArchiveConversation={noop} onUnarchiveConversation={noop} onTrashConversation={noop} onRestoreConversation={noop} onPurgeConversation={noop} onDeleteProject={noop} onNavigate={noop} onExitManagement={noop} onExpand={noop} onTheme={noop} onSettings={noop} /><main>侧栏交互夹具：合成项目、对话与通知</main></div>
}
function Fixture() {
  const [route, setRoute] = useState<any>({ view: query.get('view') === 'remote' ? 'knowledge-detail' : query.get('view') === 'detail' ? 'local-knowledge-detail' : query.get('view') === 'home' ? 'local-knowledge-home' : query.get('view') === 'documents' ? 'local-knowledge-documents' : 'local-knowledge', id: query.get('view') === 'remote' ? 'fixture-remote' : 'local-product-docs', documentId: query.get('documentId') ?? undefined })
  const navigate = (view: any, id?: string, options?: any) => { setRoute({ view, id, documentId: options?.documentId }); (window as any).__libraryFixture.route = { view, id, documentId: options?.documentId } }
  const localViews: Record<string, any> = { 'local-knowledge': 'list', 'local-files': 'files', 'local-knowledge-home': 'home', 'local-knowledge-detail': 'detail', 'local-knowledge-documents': 'documents', 'local-knowledge-jobs': 'jobs', 'local-knowledge-settings': 'settings', 'local-knowledge-models': 'models', 'local-knowledge-retrieval': 'retrieval' }
  return <div className="fox-app" style={{ height: '100vh', display: 'flex', flexDirection: 'column', background: 'var(--background)', fontFamily: 'var(--font-sans)' }}><div style={{ height: 30, display: 'flex', alignItems: 'center', padding: '0 16px', borderBottom: '1px solid var(--border)', color: 'var(--muted-foreground)', fontSize: 10 }}>Fox · 资料库组件夹具（合成数据，未连接真实 Yuxi）</div><div style={{ display: 'flex', flex: 1, minHeight: 0 }}><aside className="fox-sidebar" style={{ position: 'relative', width: 226, minWidth: 226, display: 'flex', flexDirection: 'column', height: '100%', background: 'var(--sidebar)', borderRight: '1px solid var(--border)', padding: '18px 10px' }}><div style={{ fontSize: 18, fontWeight: 600, padding: '6px 12px 22px' }}>小狐 <span style={{ fontSize: 11, color: 'var(--muted-foreground)', marginLeft: 8 }}>知识</span></div><LocalKnowledgeSidebarNavigation collapsed={false} activeView={route.view} activeEntityId={route.id} activeDocumentId={route.documentId} gateway={gateway} navigate={navigate} onCreate={() => { navigate('local-knowledge'); (window as any).__libraryFixture.createRequest = Date.now() }} /><div style={{ marginTop: 'auto', padding: 14 }}><img src="/mascot/fox/status/fox_give_flowers.png" alt="小狐" style={{ width: 54 }} /></div></aside><main style={{ flex: 1, minWidth: 0, overflow: 'auto' }}>{route.view === 'knowledge-detail' ? <KnowledgeDetailPage sidebarCollapsed={false} onSidebar={() => {}} navigate={navigate} knowledgeId={route.id} initialDocumentId={route.documentId} onAskKnowledge={() => operations.push({ action: 'ask-remote' })} /> : localViews[route.view] ? <LocalKnowledgeWorkspace view={localViews[route.view]} knowledgeBaseId={route.id} documentId={route.documentId} gateway={gateway} onBack={() => navigate('local-knowledge')} onNavigate={(view, id, documentId) => { const entry = Object.entries(localViews).find(([, value]) => value === view); navigate(entry?.[0] ?? 'local-knowledge', id, { documentId }) }} onNavigateWorkspace={navigate} onAskDocument={(payload) => operations.push({ action: 'ask-local', payload })} /> : <div style={{ padding: 30 }}>已导航到：{route.view}<button onClick={() => navigate('local-knowledge')}>返回资料库</button></div>}</main></div></div>
}
createRoot(document.getElementById('root')!).render(<div className="fox-shell" style={{ display: 'block' }}>{query.has('sidebar') ? <TooltipProvider><SidebarFixture /></TooltipProvider> : <Fixture />}</div>)
