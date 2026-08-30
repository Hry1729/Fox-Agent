import { useEffect, useState } from 'react'
import { Clock3, Cpu, Database, FileStack, House, LoaderCircle, Plus } from 'lucide-react'
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

export function LocalKnowledgeSidebarNavigation({
  collapsed,
  activeView,
  activeEntityId,
  activeDocumentId,
  navigate,
  onCreate,
  gateway = defaultLocalKnowledgeGateway,
}: {
  collapsed: boolean
  activeView: WorkspaceView
  activeEntityId?: string | null
  activeDocumentId?: string | null
  navigate: NavigateWorkspace
  onCreate?: () => void
  gateway?: LocalKnowledgeGateway
}) {
  const [bases, setBases] = useState<LocalKnowledgeBase[]>([])
  const [documents, setDocuments] = useState<LocalKnowledgeDocument[]>([])
  const [documentsLoading, setDocumentsLoading] = useState(false)
  const [documentsError, setDocumentsError] = useState(false)
  const [error, setError] = useState(false)

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

  const knowledgeBaseActive = knowledgeBaseViews.includes(activeView)
    || (activeView === 'local-knowledge-jobs' && Boolean(activeEntityId))
  const globalJobsActive = activeView === 'local-knowledge-jobs' && !activeEntityId

  return (
    <>
      <div className="fox-sidebar-primary fox-local-knowledge-sidebar-primary">
        <button className={`fox-sidebar-command ${activeView === 'local-knowledge-home' ? 'is-active' : ''}`} onClick={() => navigate('local-knowledge-home')}><House size={16} /><span>最近</span></button>
        <button className={`fox-sidebar-command ${activeView === 'local-files' ? 'is-active' : ''}`} onClick={() => navigate('local-files')}><FileStack size={16} /><span>本地文件</span></button>
        <button className={`fox-sidebar-command ${knowledgeBaseActive ? 'is-active' : ''}`} onClick={() => navigate('local-knowledge')}><Database size={16} /><span>本地知识库</span></button>
        <button className={`fox-sidebar-command ${activeView === 'local-knowledge-models' ? 'is-active' : ''}`} onClick={() => navigate('local-knowledge-models')}><Cpu size={16} /><span>向量模型</span></button>
        <button className={`fox-sidebar-command ${globalJobsActive ? 'is-active' : ''}`} onClick={() => navigate('local-knowledge-jobs')}><Clock3 size={16} /><span>任务中心</span></button>
      </div>
      {!collapsed && (
        <ScrollArea className="fox-sidebar-scroll">
          <section className="fox-sidebar-section fox-local-knowledge-sidebar-section">
            <div className="fox-section-head"><span>我的知识库</span><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="新建知识库" onClick={() => { if (onCreate) onCreate(); else navigate('local-knowledge') }}><Plus size={14} /></Button></div>
            <div className="fox-local-knowledge-sidebar-list">
              {bases.map((base) => {
                const active = activeEntityId === base.id && knowledgeBaseViews.includes(activeView)
                return <div className="fox-local-knowledge-sidebar-base" key={base.id}>
                  <button type="button" className={`fox-sidebar-tree-row fox-local-knowledge-sidebar-row ${active ? 'is-active' : ''}`} title={base.name} onClick={() => navigate('local-knowledge-detail', base.id)}>
                    <Database size={14} />
                    <span>{base.name}</span>
                    <small>{base.documentCount}</small>
                  </button>
                  {active && <div className="fox-local-knowledge-sidebar-documents" aria-label={`${base.name}文件列表`}>
                    {documentsLoading && <p><LoaderCircle className="animate-spin" />正在加载文件...</p>}
                    {!documentsLoading && documents.map((document) => <button key={document.id} type="button" className={activeDocumentId === document.id ? 'is-active' : undefined} title={document.relativePath} onClick={() => navigate('local-knowledge-documents', base.id, { documentId: document.id })}><LocalFileTypeIcon extension={document.extension} /><span>{document.name}</span></button>)}
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
