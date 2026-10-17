import { useEffect, useState } from 'react'
import { Plus } from 'lucide-react'
import { Home, Library, ListTodo, Orbit } from '@/components/fox-ui/animated-icons'
import { SidebarMotionItem } from '@/features/chat/shell/fox-sidebar-motion'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { useKnowledgeBases } from '@/features/knowledge/use-knowledge'
import { useYuxiService } from '@/features/settings/use-yuxi-service'
import { useYuxiUser } from '@/features/settings/use-yuxi-user'
import type { NavigateWorkspace, WorkspaceView } from '@/features/workspace/types'
import { LibrarySidebarTree } from './library-sidebar-tree'
import { LOCAL_FOLDERS_CHANGED } from './local-folder-pages'
import { defaultLocalKnowledgeGateway } from './tauri-gateway'
import type { LocalKnowledgeBase, LocalKnowledgeFileSource, LocalKnowledgeGateway } from './model'

export function LocalKnowledgeSidebarNavigation({ collapsed, activeView, activeEntityId, navigate, hideLibrary = false, onCreate, gateway = defaultLocalKnowledgeGateway }: {
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
  const [sources, setSources] = useState<LocalKnowledgeFileSource[]>([])
  const [sourceError, setSourceError] = useState(false)
  useEffect(() => {
    let disposed = false
    const load = () => { void gateway.listFileSources().then((items) => { if (!disposed) { setSources(items); setSourceError(false) } }).catch(() => { if (!disposed) setSourceError(true) }) }
    load()
    window.addEventListener(LOCAL_FOLDERS_CHANGED, load)
    return () => { disposed = true; window.removeEventListener(LOCAL_FOLDERS_CHANGED, load) }
  }, [activeView, gateway])
  const [error, setError] = useState(false)
  const yuxi = useYuxiService()
  const user = useYuxiUser(Boolean(yuxi.service?.credentialConfigured))
  const remote = useKnowledgeBases(Boolean(user.user))
  useEffect(() => {
    let disposed = false
    void gateway.listKnowledgeBases().then((items) => { if (!disposed) { setBases(items); setError(false) } }).catch(() => { if (!disposed) setError(true) })
    return () => { disposed = true }
  }, [activeView, gateway])
  const libraryActive = activeView === 'knowledge' || activeView === 'knowledge-detail' || activeView === 'knowledge-graph' || activeView === 'local-files' || (activeView.startsWith('local-knowledge') && !['local-knowledge-home', 'local-knowledge-models', 'local-knowledge-jobs'].includes(activeView))
  return <>
    <div className="fox-sidebar-primary fox-local-knowledge-sidebar-primary">
      {[
        { view: 'local-knowledge-home' as const, label: '最近', icon: Home, active: activeView === 'local-knowledge-home' },
        { view: 'local-knowledge' as const, label: '资料库', icon: Library, active: libraryActive },
        { view: 'local-knowledge-models' as const, label: '向量模型', icon: Orbit, active: activeView === 'local-knowledge-models' },
        { view: 'local-knowledge-jobs' as const, label: '任务中心', icon: ListTodo, active: activeView === 'local-knowledge-jobs' },
      ].map(({ view, label, icon: Icon, active }) => <SidebarMotionItem key={view}><button type="button" className={`fox-sidebar-command ${active ? 'is-active' : ''}`} aria-label={label} aria-current={active ? 'page' : undefined} onClick={() => navigate(view)}><Icon size={18} /><span>{label}</span></button></SidebarMotionItem>)}
    </div>
    {!collapsed && !hideLibrary && <ScrollArea className="fox-sidebar-scroll"><section className="fox-sidebar-section fox-local-knowledge-sidebar-section"><div className="fox-section-head"><span>本机</span><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="新建资料库" onClick={() => { if (onCreate) onCreate(); else navigate('local-knowledge') }}><Plus size={14} /></Button></div><div className="fox-local-knowledge-sidebar-list">
      {bases.map((base) => <LibrarySidebarTree key={base.id} id={base.id} name={base.name} count={base.documentCount} active={activeEntityId === base.id && activeView.startsWith('local-knowledge')} source="本机" onNavigate={() => navigate('local-knowledge-detail', base.id)} />)}
      {error && <p className="fox-project-empty">本机资料库列表暂不可用</p>}
      {sources.map((source) => <LibrarySidebarTree key={source.id} id={`local-folder:${source.id}`} name={source.displayName} count={source.fileCount} active={activeEntityId === source.id && activeView === 'local-files'} source="本机文件夹" onNavigate={() => navigate('local-files', source.id)} />)}
      {sourceError && <p className="fox-project-empty">文件夹列表暂不可用</p>}
      {!error && !sourceError && !bases.length && !sources.length && <p className="fox-project-empty">暂无本机资料</p>}
    </div></section>{remote.items.length > 0 && <section className="fox-sidebar-section fox-local-knowledge-sidebar-section"><div className="fox-section-head"><span>远程</span></div><div className="fox-local-knowledge-sidebar-list">{remote.items.map((base) => <LibrarySidebarTree key={`remote:${base.id}`} id={`remote:${base.id}`} name={base.name} count={base.fileCount} active={activeEntityId === base.id && activeView.startsWith('knowledge')} source="远程" onNavigate={() => navigate('knowledge-detail', base.id)} />)}</div></section>}</ScrollArea>}
  </>
}
