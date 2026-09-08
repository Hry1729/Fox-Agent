import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { ChevronRight, Database, Files, Folder, FolderOpen, House, ListTodo, LoaderCircle, Orbit, Plus } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import type { NavigateWorkspace, WorkspaceView } from '@/features/workspace/types'
import { defaultLocalKnowledgeGateway } from './tauri-gateway'
import { LocalFileTypeIcon } from './local-file-type-icon'
import type { LocalKnowledgeBase, LocalKnowledgeDocument, LocalKnowledgeGateway } from './model'

const knowledgeBaseViews: WorkspaceView[] = [
  'local-knowledge',
  'local-knowledge-detail',
  'local-knowledge-documents',
  'local-knowledge-import',
  'local-knowledge-retrieval',
]

type KnowledgeTreeNode = KnowledgeTreeFolder | KnowledgeTreeDocument

interface KnowledgeTreeFolder {
  kind: 'folder'
  key: string
  name: string
  children: KnowledgeTreeNode[]
}

interface KnowledgeTreeDocument {
  kind: 'document'
  key: string
  name: string
  document: LocalKnowledgeDocument
}

function sortKnowledgeTree(nodes: KnowledgeTreeNode[]) {
  nodes.sort((left, right) => {
    if (left.kind !== right.kind) return left.kind === 'folder' ? -1 : 1
    return left.name.localeCompare(right.name, 'zh-CN')
  })
  nodes.forEach((node) => {
    if (node.kind === 'folder') sortKnowledgeTree(node.children)
  })
}

function buildKnowledgeTree(documents: LocalKnowledgeDocument[]): KnowledgeTreeNode[] {
  const root: KnowledgeTreeFolder = { kind: 'folder', key: '', name: '', children: [] }
  documents.forEach((document) => {
    const pathParts = document.relativePath.replace(/\\/g, '/').split('/').filter(Boolean)
    const folderParts = pathParts.length > 1 ? pathParts.slice(0, -1) : []
    let parent = root
    folderParts.forEach((part, index) => {
      const key = folderParts.slice(0, index + 1).join('/')
      let folder = parent.children.find((node): node is KnowledgeTreeFolder => node.kind === 'folder' && node.key === key)
      if (!folder) {
        folder = { kind: 'folder', key, name: part, children: [] }
        parent.children.push(folder)
      }
      parent = folder
    })
    parent.children.push({ kind: 'document', key: `document:${document.id}`, name: document.name, document })
  })
  sortKnowledgeTree(root.children)
  return root.children
}

function collectFolderKeys(nodes: KnowledgeTreeNode[], result = new Set<string>()) {
  nodes.forEach((node) => {
    if (node.kind !== 'folder') return
    result.add(node.key)
    collectFolderKeys(node.children, result)
  })
  return result
}

export function LocalKnowledgeSidebarNavigation({
  collapsed,
  activeView,
  activeEntityId,
  activeDocumentId,
  navigate,
  hideLibrary = false,
  onCreate,
  gateway = defaultLocalKnowledgeGateway,
}: {
  collapsed: boolean
  activeView: WorkspaceView
  activeEntityId?: string | null
  activeDocumentId?: string | null
  navigate: NavigateWorkspace
  hideLibrary?: boolean
  onCreate?: () => void
  gateway?: LocalKnowledgeGateway
}) {
  const [bases, setBases] = useState<LocalKnowledgeBase[]>([])
  const [documents, setDocuments] = useState<LocalKnowledgeDocument[]>([])
  const [documentsLoading, setDocumentsLoading] = useState(false)
  const [documentsError, setDocumentsError] = useState(false)
  const [error, setError] = useState(false)
  const [expandedFolders, setExpandedFolders] = useState<Set<string>>(new Set())

  useEffect(() => {
    let disposed = false
    void gateway.listKnowledgeBases()
      .then((items) => {
        if (disposed) return
        setBases(items)
        setError(false)
      })
      .catch(() => {
        if (!disposed) setError(true)
      })
    return () => { disposed = true }
  }, [activeView, gateway])

  useEffect(() => {
    if (!activeEntityId || !knowledgeBaseViews.includes(activeView)) {
      setDocuments([])
      setDocumentsError(false)
      return
    }
    let disposed = false
    setDocumentsLoading(true)
    void gateway.listDocuments(activeEntityId)
      .then((page) => {
        if (disposed) return
        setDocuments(page.items)
        setDocumentsError(false)
      })
      .catch(() => {
        if (!disposed) setDocumentsError(true)
      })
      .finally(() => {
        if (!disposed) setDocumentsLoading(false)
      })
    return () => { disposed = true }
  }, [activeEntityId, activeView, gateway])

  const documentTree = useMemo(() => buildKnowledgeTree(documents), [documents])

  useEffect(() => {
    setExpandedFolders(collectFolderKeys(documentTree))
  }, [activeEntityId, documentTree])

  const knowledgeBaseActive = knowledgeBaseViews.includes(activeView)
    || (activeView === 'local-knowledge-jobs' && Boolean(activeEntityId))
  const globalJobsActive = activeView === 'local-knowledge-jobs' && !activeEntityId
  const toggleFolder = (key: string) => {
    setExpandedFolders((current) => {
      const next = new Set(current)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }
  const renderTree = (nodes: KnowledgeTreeNode[]): ReactNode => nodes.map((node) => {
    if (node.kind === 'document') {
      return <button key={node.key} type="button" className={`fox-local-knowledge-tree-document ${activeDocumentId === node.document.id ? 'is-active' : ''}`} title={node.document.relativePath} onClick={() => navigate('local-knowledge-documents', node.document.knowledgeBaseId, { documentId: node.document.id })}><LocalFileTypeIcon extension={node.document.extension} /><span>{node.name}</span></button>
    }
    const expanded = expandedFolders.has(node.key)
    return <div className="fox-local-knowledge-tree-folder" key={node.key}>
      <button type="button" className="fox-local-knowledge-tree-folder-row" aria-expanded={expanded} title={node.key} onClick={() => toggleFolder(node.key)}><ChevronRight className="fox-local-knowledge-tree-chevron" /><span className="fox-local-knowledge-tree-folder-icon">{expanded ? <FolderOpen /> : <Folder />}</span><span>{node.name}</span></button>
      {expanded && <div className="fox-local-knowledge-tree-children">{renderTree(node.children)}</div>}
    </div>
  })

  return (
    <>
      <div className="fox-sidebar-primary fox-local-knowledge-sidebar-primary">
        <button className={`fox-sidebar-command ${activeView === 'local-knowledge-home' ? 'is-active' : ''}`} onClick={() => navigate('local-knowledge-home')}><House size={18} /><span>最近</span></button>
        <button className={`fox-sidebar-command ${activeView === 'local-files' ? 'is-active' : ''}`} onClick={() => navigate('local-files')}><Files size={18} /><span>本地文件</span></button>
        <button className={`fox-sidebar-command ${knowledgeBaseActive ? 'is-active' : ''}`} onClick={() => navigate('local-knowledge')}><Database size={18} /><span>本地知识库</span></button>
        <button className={`fox-sidebar-command ${activeView === 'local-knowledge-models' ? 'is-active' : ''}`} onClick={() => navigate('local-knowledge-models')}><Orbit size={18} /><span>向量模型</span></button>
        <button className={`fox-sidebar-command ${globalJobsActive ? 'is-active' : ''}`} onClick={() => navigate('local-knowledge-jobs')}><ListTodo size={18} /><span>任务中心</span></button>
      </div>
      {!collapsed && !hideLibrary && (
        <ScrollArea className="fox-sidebar-scroll">
          <section className="fox-sidebar-section fox-local-knowledge-sidebar-section">
            <div className="fox-section-head"><span>我的知识库</span><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="新建知识库" onClick={() => { if (onCreate) onCreate(); else navigate('local-knowledge') }}><Plus size={14} /></Button></div>
            <div className="fox-local-knowledge-sidebar-list">
              {bases.map((base) => {
                const active = activeEntityId === base.id && knowledgeBaseViews.includes(activeView)
                return <div className={`fox-local-knowledge-sidebar-base ${active ? 'is-active' : ''}`} key={base.id}>
                  <button type="button" className={`fox-sidebar-tree-row fox-local-knowledge-sidebar-row ${active ? 'is-active' : ''}`} title={base.name} onClick={() => navigate('local-knowledge-detail', base.id)}>
                    <Database size={14} />
                    <span>{base.name}</span>
                    <small>{base.documentCount}</small>
                  </button>
                  {active && <div className="fox-local-knowledge-sidebar-documents" aria-label={`${base.name}文件列表`}>
                    {documentsLoading && <p><LoaderCircle className="animate-spin" />正在加载文件...</p>}
                    {!documentsLoading && renderTree(documentTree)}
                    {!documentsLoading && !documentsError && documents.length === 0 && <p>暂无文件</p>}
                    {!documentsLoading && documentsError && <p>文件列表暂不可用</p>}
                  </div>}
                </div>
              })}
              {!error && bases.length === 0 && <p className="fox-project-empty">暂无本地知识库</p>}
              {error && <p className="fox-project-empty">知识库列表暂不可用</p>}
            </div>
          </section>
        </ScrollArea>
      )}
    </>
  )
}
