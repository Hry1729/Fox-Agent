import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { Folder } from 'lucide-react'
import { Files, SubFiles, FolderItem, FolderTrigger, FolderPanel, FileItem } from '@/components/animate-ui/components/base/files'
import { useLibraryBrowser, navigateLibraryFolder } from './library-browser-state'
import { libraryParentPath } from './library-file-model'

export function LibrarySidebarTree({ id, name, count, active, source, onNavigate }: {
  id: string; name: string; count: number; active: boolean; source: '本机' | '远程' | '本机文件夹'; onNavigate: () => void
}) {
  const browser = useLibraryBrowser(id)
  const root = `library:${id}`
  const [open, setOpen] = useState<string[]>(active ? [root] : [])
  const children = useMemo(() => {
    const index = new Map<string, typeof browser.folders>()
    for (const folder of browser.folders) {
      const parent = libraryParentPath(folder.path)
      const siblings = index.get(parent) ?? []
      siblings.push(folder)
      index.set(parent, siblings)
    }
    return index
  }, [browser.folders])
  useEffect(() => {
    if (!active) { setOpen([]); return }
    const parts = browser.path.split('/').filter(Boolean)
    const ancestors = [root, ...parts.map((_, index) => parts.slice(0, index + 1).join('/'))]
    setOpen((current) => [...new Set([...current, ...ancestors])])
  }, [active, browser.path, root])
  const select = (path: string) => { navigateLibraryFolder(id, path); onNavigate() }
  const render = (parent: string): ReactNode => (children.get(parent) ?? []).map((folder) => {
    const label = <><span>{folder.path.split('/').pop()}</span><small>{folder.count}</small></>
    const title = `${folder.path} · ${folder.count} 个文件（含子文件夹）`
    const selected = active && browser.path === folder.path
    return children.has(folder.path)
      ? <FolderItem key={folder.path} value={folder.path}><FolderTrigger triggerProps={{ title, 'aria-current': selected ? 'page' : undefined, onClick: () => select(folder.path) }}>{label}</FolderTrigger><FolderPanel>{open.includes(folder.path) && <SubFiles open={open} onOpenChange={setOpen}>{render(folder.path)}</SubFiles>}</FolderPanel></FolderItem>
      : <FileItem key={folder.path} icon={Folder} title={title} aria-current={selected ? 'page' : undefined} onClick={() => select(folder.path)}>{label}</FileItem>
  })
  return <Files open={active ? open : []} onOpenChange={setOpen} className={`fox-animate-file-tree fox-library-sidebar-tree${active ? ' is-active' : ''}`} aria-label={`${name}的文件夹`}>
    <FolderItem value={root}><FolderTrigger triggerProps={{ className: active ? 'is-library-active' : undefined, title: `${name} · ${source}`, 'aria-current': active && !browser.path ? 'page' : undefined, onClick: () => select('') }}><span>{name}</span><small>{count}</small></FolderTrigger>
      <FolderPanel>{active && <div className="fox-library-tree-scroll" role="region" aria-label={`${name}的目录`} tabIndex={0}><SubFiles open={open} onOpenChange={setOpen}>{render('')}{active && !browser.folders.length && <span className="fox-library-sidebar-empty">暂无文件夹</span>}</SubFiles></div>}</FolderPanel>
    </FolderItem>
  </Files>
}
