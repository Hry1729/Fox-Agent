import FolderComponent from '@/components/ui/folder-component'
import { Fragment, useCallback, useEffect, useMemo, useState, type ReactNode } from 'react'
import { ArrowLeft, Folder, FolderOpen, MoreHorizontal, Pencil, Laptop, RefreshCw, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { ExpandingSearch } from '@/components/arc/expanding-search/expanding-search'
import { LibraryFileManager, LibraryViewSwitch } from './library-file-manager'
import { navigateLibraryFolder, publishLibraryFolders, setLibraryView, useLibraryBrowser } from './library-browser-state'
import type { LocalKnowledgeCatalogFile, LocalKnowledgeFileSource, LocalKnowledgeGateway } from './model'
import type { LibraryFile } from './library-file-model'

export const LOCAL_FOLDERS_CHANGED = 'fox:local-folders-changed'
const changed = () => window.dispatchEvent(new Event(LOCAL_FOLDERS_CHANGED))
const errorMessage = (cause: unknown) => cause instanceof Error ? cause.message : String(cause)

export function LocalFolderSection({ gateway, query, onOpen, libraryCount = 0, matchedLibraryCount = 0, librariesLoading = false, children }: { gateway: LocalKnowledgeGateway; query: string; onOpen: (id: string) => void; libraryCount?: number; matchedLibraryCount?: number; librariesLoading?: boolean; children?: ReactNode }) {
  const [sources, setSources] = useState<LocalKnowledgeFileSource[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [editor, setEditor] = useState<{ source: LocalKnowledgeFileSource; remove: boolean } | null>(null)
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const load = useCallback(async () => { setLoading(true); try { setSources(await gateway.listFileSources()); setError(null) } catch (cause) { setError(errorMessage(cause)) } finally { setLoading(false) } }, [gateway])
  useEffect(() => { void load(); window.addEventListener(LOCAL_FOLDERS_CHANGED, load); return () => window.removeEventListener(LOCAL_FOLDERS_CHANGED, load) }, [load])
  const mutate = async (action: () => Promise<unknown>) => {
    setBusy(true); setError(null)
    try { await action(); setEditor(null); changed() } catch (cause) { setError(errorMessage(cause)) } finally { setBusy(false) }
  }
  const visible = sources.filter((source) => `${source.displayName} ${source.description ?? ''} ${source.rootPath}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()))
  return <section className="fox-library-source-section" aria-label="本机">
    <div className="fox-local-folder-group-head"><h2 className="fox-library-group-title"><Laptop />本机<small>{sources.length + libraryCount}</small></h2></div>
    {loading && <p role="status">正在读取文件夹…</p>}
    {error && !editor && <p role="alert">{error} <Button variant="ghost" size="sm" onClick={() => void load()}>重试</Button></p>}
    {!loading && !librariesLoading && !error && !visible.length && !matchedLibraryCount && <p className="fox-local-folder-empty">{query.trim() ? '没有匹配的本机资料。' : '还没有本机资料。点击“新建资料库”添加。'}</p>}
    <div className="fox-kb-cover-grid">{visible.map((source) => <article className="fox-kb-cover-card" key={source.id}>
      <button type="button" className="fox-kb-card-open" aria-label={`打开本机文件夹${source.displayName}`} onClick={() => onOpen(source.id)}>
      <span className="fox-kb-cover" aria-hidden="true"><FolderComponent color="theme" size="xs" /></span>
      <span className="fox-kb-cover-body"><strong title={source.displayName}>{source.displayName}<span className="fox-library-source-label"><Laptop />本机</span></strong><p title={source.description || undefined}>{source.description?.trim() || '无简介'}</p><span className="fox-kb-cover-meta">{source.fileCount} 份资料</span></span>
      </button>
      <span className="fox-kb-cover-menu" onClick={(event) => event.stopPropagation()} onKeyDown={(event) => event.stopPropagation()}><DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-kb-cover-more" aria-label={`${source.displayName} 的更多操作`}><MoreHorizontal /></Button></DropdownMenuTrigger><DropdownMenuContent align="end" className="fox-library-folder-menu">
        <DropdownMenuLabel>文件位置</DropdownMenuLabel><div className="fox-library-menu-path" title={source.rootPath}>{source.rootPath}</div><DropdownMenuSeparator />
        <DropdownMenuItem disabled={busy} onSelect={() => void mutate(() => gateway.rescanFileSource(source.id))}><RefreshCw />刷新文件</DropdownMenuItem>
        <DropdownMenuItem disabled={busy} onSelect={() => { setError(null); setName(source.displayName); setDescription(source.description ?? ''); setEditor({ source, remove: false }) }}><Pencil />编辑信息</DropdownMenuItem>
        <DropdownMenuItem disabled={busy} onSelect={() => { setError(null); setEditor({ source, remove: true }) }}><Trash2 />移除文件夹</DropdownMenuItem>
      </DropdownMenuContent></DropdownMenu></span>
    </article>)}{children}</div>
    <Dialog open={Boolean(editor)} onOpenChange={(open) => { if (!open && !busy) { setEditor(null); setError(null) } }}><DialogContent className="fox-local-kb-create-dialog fox-kb-settings-dialog" showCloseButton={!busy}><DialogHeader><DialogTitle>{editor?.remove ? '移除文件夹' : '编辑信息'}</DialogTitle><DialogDescription>{editor?.remove ? '只取消 Fox 中的收录，磁盘上的文件会保留。' : '修改 Fox 中的名称和简介，磁盘上的文件夹保持不变。'}</DialogDescription></DialogHeader>
      <form className="fox-kb-settings-form" onSubmit={(event) => { event.preventDefault(); if (editor && !busy && (editor.remove || name.trim())) void mutate(() => editor.remove ? gateway.removeFileSource(editor.source.id) : gateway.updateFileSource(editor.source.id, name.trim(), description.trim())) }}>
      {editor?.remove ? <p>{editor.source.displayName}</p> : <><div><label htmlFor="local-folder-edit-name">名称 <span>*</span></label><Input id="local-folder-edit-name" autoFocus value={name} maxLength={80} disabled={busy} onChange={(event) => setName(event.target.value)} /></div><div><label htmlFor="local-folder-edit-description">简介 <small>（可选）</small></label><Textarea id="local-folder-edit-description" className="fox-local-kb-create-textarea" value={description} disabled={busy} onChange={(event) => setDescription(event.target.value)} /></div></>}
      {error && <p role="alert">{error}</p>}
      <DialogFooter className="fox-local-folder-edit-actions"><Button type="button" variant="outline" disabled={busy} onClick={() => { setEditor(null); setError(null) }}>取消</Button><Button type="submit" disabled={busy || (!editor?.remove && !name.trim())} variant={editor?.remove ? 'destructive' : 'default'}>{busy ? '处理中…' : editor?.remove ? '移除' : '保存'}</Button></DialogFooter></form>
    </DialogContent></Dialog>
  </section>
}

export function LocalFolderDetailPage({ gateway, sourceId, onBack, renderPreview }: { gateway: LocalKnowledgeGateway; sourceId: string; onBack: () => void; renderPreview: (file: LocalKnowledgeCatalogFile | null, close: () => void) => ReactNode }) {
  const browserId = `local-folder:${sourceId}`
  const browser = useLibraryBrowser(browserId)
  const [source, setSource] = useState<LocalKnowledgeFileSource | null>(null)
  const [files, setFiles] = useState<LocalKnowledgeCatalogFile[]>([])
  const [preview, setPreview] = useState<LocalKnowledgeCatalogFile | null>(null)
  const [query, setQuery] = useState('')
  const [loading, setLoading] = useState(true)
  const [refreshing, setRefreshing] = useState(false)
  const [revision, setRevision] = useState(0)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let disposed = false
    let timer: ReturnType<typeof setTimeout> | undefined
    setError(null)
    const readFiles = async () => {
      const all: LocalKnowledgeCatalogFile[] = []
      while (!disposed) {
        const batch = await gateway.listLocalFiles({ sourceId, offset: all.length, limit: 2000 })
        all.push(...batch)
        if (batch.length < 2000) break
      }
      return all
    }
    const sync = async () => {
      setRefreshing(true)
      try {
        const current = await gateway.rescanFileSource(sourceId)
        // The gateway invalidates catalog caches before this read.
        const updated = disposed ? [] : await readFiles()
        changed()
        if (disposed) return
        setSource(current)
        setFiles((previous) => JSON.stringify(previous) === JSON.stringify(updated) ? previous : updated)
        setPreview((previous) => {
          if (!previous) return null
          const next = updated.find((file) => file.id === previous.id) ?? null
          return JSON.stringify(previous) === JSON.stringify(next) ? previous : next
        })
      } catch (cause) {
        if (!disposed) setError(`文件夹更新失败，当前显示上次记录：${errorMessage(cause)}`)
      } finally { if (!disposed) setRefreshing(false) }
    }
    void (async () => {
      try {
        const sources = await gateway.listFileSources()
        const current = sources.find((item) => item.id === sourceId)
        if (!current) throw new Error('这个文件夹已移除，请返回资料库重新添加。')
        const all = await readFiles()
        if (!disposed) {
          setSource(current); setFiles(all); setLoading(false)
          timer = setTimeout(() => { if (!disposed) void sync() }, 150)
        }
      } catch (cause) { if (!disposed) setError(errorMessage(cause)) } finally { if (!disposed) setLoading(false) }
    })()
    return () => { disposed = true; clearTimeout(timer) }
  }, [gateway, sourceId, revision])
  const entries = useMemo<LibraryFile[]>(() => files.map((file) => ({ id: file.id, name: file.name, path: file.relativePath, size: file.sizeBytes, modifiedAt: file.modifiedAt, status: '本机文件' })), [files])
  useEffect(() => { publishLibraryFolders(browserId, entries) }, [browserId, entries])
  const refresh = () => setRevision((value) => value + 1)
  return <div className="fox-local-kb-page fox-local-kb-detail-page"><div className="fox-local-kb-content fox-local-kb-detail-content">
    <header className="fox-kb-hub-head fox-library-detail-topbar"><Button aria-label="返回资料库" className="fox-local-kb-back" variant="ghost" size="icon" onClick={onBack}><ArrowLeft /></Button>
      <nav className="fox-library-breadcrumb" aria-label="本机文件夹路径"><button type="button" title={source?.rootPath} onClick={() => navigateLibraryFolder(browserId, '')}>{source?.displayName ?? '本机文件夹'}</button>{browser.path.split('/').filter(Boolean).map((name, index, parts) => <Fragment key={index}><span aria-hidden="true">/</span><button type="button" aria-current={index === parts.length - 1 ? 'page' : undefined} onClick={() => navigateLibraryFolder(browserId, parts.slice(0, index + 1).join('/'))}>{name}</button></Fragment>)}</nav>
      <div className="fox-kb-hub-actions"><div className="fox-kb-search-control-slot"><ExpandingSearch reserveSpace clearLabel="清空搜索" className="fox-kb-expanding-search" anchor="end" label="搜索本机文件夹" placeholder="搜索文件名称" items={[]} value={query} onValueChange={setQuery} onSearch={setQuery} showResults={false} keyboardShortcut expandedWidth={300} /></div><Button variant="ghost" size="icon" aria-label="刷新文件夹" disabled={loading || refreshing} onClick={() => void refresh()}><RefreshCw className={refreshing ? 'animate-spin' : ''} /></Button><LibraryViewSwitch view={browser.view} onChange={(view) => setLibraryView(browserId, view)} /></div>
    </header>
    {error && <p role="alert">{error} <Button variant="ghost" onClick={() => { if (source) void refresh(); else setRevision((value) => value + 1) }}>重试</Button></p>}
    {loading ? <p role="status">正在读取文件夹…</p> : source && !entries.length ? <div className="fox-library-empty"><FolderOpen /><b>没有可显示的文档</b><p>支持文本、PDF、Word、表格和演示文稿。添加文档后刷新文件夹。</p></div> : source && <LibraryFileManager contentOnly view={browser.view} files={entries} path={browser.path} onPathChange={(path) => navigateLibraryFolder(browserId, path)} searchQuery={query} onSearchQueryChange={setQuery} onOpen={(entry) => setPreview(files.find((file) => file.id === entry.id) ?? null)} onReveal={(entry) => { const file = files.find((item) => item.id === entry.id); if (file) void gateway.revealLocalFile(file.id).catch((cause) => setError(errorMessage(cause))) }} footer={<span>{files.length} 个文件 · {source.rootPath}</span>} />}
  </div>{renderPreview(preview, () => setPreview(null))}</div>
}
