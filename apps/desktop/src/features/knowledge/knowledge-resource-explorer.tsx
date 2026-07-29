import { useEffect, useMemo, useState, type ReactNode } from 'react'
import {
  ChevronDown,
  ChevronRight,
  File,
  FileCode2,
  FileJson,
  FileSpreadsheet,
  FileText,
  Folder,
  FolderOpen,
  Image,
} from 'lucide-react'
import { ScrollArea } from '@/components/ui/scroll-area'
import type { KnowledgeDocumentRecord } from '@/features/conversations/model/types'
import type { NavigateWorkspace } from '@/features/workspace/types'
import { useKnowledgeDetail } from './use-knowledge'

interface DocumentTreeNode {
  document: KnowledgeDocumentRecord
  children: DocumentTreeNode[]
}

export function KnowledgeResourceExplorer({
  knowledgeId,
  selectedDocumentId,
  navigate,
}: {
  knowledgeId?: string | null
  selectedDocumentId?: string | null
  navigate: NavigateWorkspace
}) {
  const resource = useKnowledgeDetail(knowledgeId ?? null)
  const [rootExpanded, setRootExpanded] = useState(true)
  const [expandedFolders, setExpandedFolders] = useState<Set<string>>(new Set())
  const documents = resource.detail?.documents ?? []
  const tree = useMemo(() => buildDocumentTree(documents), [documents])
  const database = resource.detail?.database
  const fileCount = database?.fileCount ?? documents.filter((item) => !item.isFolder).length

  useEffect(() => {
    if (!documents.length) return
    setExpandedFolders((current) =>
      current.size
        ? current
        : new Set(documents.filter((item) => item.isFolder && !item.parentId).map((item) => item.id)),
    )
  }, [documents])

  const selectDocument = (document: KnowledgeDocumentRecord) => {
    if (document.isFolder) {
      setExpandedFolders((current) => {
        const next = new Set(current)
        if (next.has(document.id)) next.delete(document.id)
        else next.add(document.id)
        return next
      })
      return
    }
    navigate('knowledge-detail', knowledgeId ?? undefined, { documentId: document.id })
  }

  return (
    <section className="fox-knowledge-sidebar-explorer" aria-label="资源管理器">
      <div className="fox-knowledge-sidebar-divider" />
      <header>
        <span>资源管理器</span>
        <small>{fileCount}</small>
      </header>
      <ScrollArea className="fox-knowledge-sidebar-tree">
        <div className="fox-file-browser fox-sidebar-file-browser">
          <div className="fox-file-tree">
            <button
              type="button"
              className="fox-file-tree-root"
              onClick={() => setRootExpanded((value) => !value)}
              title={database?.name ?? '知识库'}
            >
              {rootExpanded ? <ChevronDown /> : <ChevronRight />}
              <FolderOpen />
              <strong>{database?.name ?? '知识库'}</strong>
            </button>
            {rootExpanded && (
              <DocumentTree
                nodes={tree}
                selectedId={selectedDocumentId ?? null}
                expandedFolders={expandedFolders}
                onSelect={selectDocument}
              />
            )}
          </div>
        </div>
        {resource.loading && <p className="fox-knowledge-sidebar-status">正在加载文件...</p>}
        {!resource.loading && !resource.error && documents.length === 0 && (
          <p className="fox-knowledge-sidebar-status">暂无文件</p>
        )}
        {resource.error && <p className="fox-knowledge-sidebar-status is-error">{resource.error}</p>}
      </ScrollArea>
    </section>
  )
}

export function knowledgeDocumentIcon(document: KnowledgeDocumentRecord): ReactNode {
  if (document.isFolder) return <Folder />
  const extension = document.name.split('.').pop()?.toLowerCase()
  if (['xls', 'xlsx', 'csv'].includes(extension ?? '')) return <FileSpreadsheet />
  if (extension === 'json') return <FileJson />
  if (['md', 'js', 'ts', 'tsx', 'py', 'rs'].includes(extension ?? '')) return <FileCode2 />
  if (['png', 'jpg', 'jpeg', 'webp', 'gif'].includes(extension ?? '')) return <Image />
  if (['pdf', 'doc', 'docx', 'txt'].includes(extension ?? '')) return <FileText />
  return <File />
}

function buildDocumentTree(documents: KnowledgeDocumentRecord[]): DocumentTreeNode[] {
  const nodes = new Map(
    documents.map((document) => [document.id, { document, children: [] as DocumentTreeNode[] }]),
  )
  const roots: DocumentTreeNode[] = []
  nodes.forEach((node) => {
    const parent = node.document.parentId ? nodes.get(node.document.parentId) : null
    if (parent) parent.children.push(node)
    else roots.push(node)
  })
  const sort = (items: DocumentTreeNode[]) => {
    items.sort(
      (a, b) =>
        Number(b.document.isFolder) - Number(a.document.isFolder) ||
        a.document.name.localeCompare(b.document.name),
    )
    items.forEach((item) => sort(item.children))
  }
  sort(roots)
  return roots
}

function DocumentTree({
  nodes,
  selectedId,
  expandedFolders,
  onSelect,
  depth = 0,
}: {
  nodes: DocumentTreeNode[]
  selectedId: string | null
  expandedFolders: Set<string>
  onSelect: (document: KnowledgeDocumentRecord) => void
  depth?: number
}) {
  return (
    <>
      {nodes.map((node) => {
        const open = expandedFolders.has(node.document.id)
        return (
          <div key={node.document.id} className="fox-file-tree-node">
            <button
              type="button"
              className={node.document.id === selectedId ? 'is-active' : ''}
              style={{ paddingLeft: `${14 + depth * 13}px` }}
              onClick={() => onSelect(node.document)}
              title={node.document.name}
            >
              {node.document.isFolder ? (
                open ? <ChevronDown className="fox-file-tree-chevron" /> : <ChevronRight className="fox-file-tree-chevron" />
              ) : (
                <span className="fox-file-tree-spacer" />
              )}
              <span className="fox-file-tree-icon">
                {node.document.isFolder ? (open ? <FolderOpen /> : <Folder />) : knowledgeDocumentIcon(node.document)}
              </span>
              <span className="fox-file-tree-name">{node.document.name}</span>
            </button>
            {node.document.isFolder && open && (
              <DocumentTree
                nodes={node.children}
                selectedId={selectedId}
                expandedFolders={expandedFolders}
                onSelect={onSelect}
                depth={depth + 1}
              />
            )}
          </div>
        )
      })}
    </>
  )
}
