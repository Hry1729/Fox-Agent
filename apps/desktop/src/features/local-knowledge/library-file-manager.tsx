import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { ArrowDown, ArrowUp, CheckCircle2, CircleAlert, Download, Eye, Grid2X2, List, Rows3, Folder, FolderOpen, FolderPlus, LoaderCircle, MoreHorizontal, Plus, RotateCcw, Search, Upload, X } from 'lucide-react'
import { FileManager, type FileManagerItem, type FileManagerView } from '@/components/corr/file-manager'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import { LocalFileTypeIcon } from './local-file-type-icon'
import { formatBytes } from './model'
import { libraryFolderCounts, libraryParentPath, normalizeLibraryPath, visibleLibraryFiles, type LibraryFile } from './library-file-model'
import './library-unification.css'

export interface LibraryFileManagerProps {
  searchQuery?: string
  onSearchQueryChange?: (query: string) => void
  contentOnly?: boolean
  view?: FileManagerView
  files: LibraryFile[]
  onOpen: (file: LibraryFile) => void
  onRemove?: (files: LibraryFile[]) => void
  onReprocess?: (files: LibraryFile[]) => void
  onReveal?: (file: LibraryFile) => void
  onDownload?: (files: LibraryFile[]) => void
  onAdd?: (path: string) => void
  onCreateFolder?: (path: string) => void
  onDropFiles?: (files: File[], path: string) => void
  onDropPaths?: (paths: string[], path: string) => void
  onPathChange?: (path: string) => void
  path?: string
  busy?: boolean
  footer?: ReactNode
}

function ProcessingState({ status }: { status?: string | null }) {
  const normalized = status?.toLowerCase()
  const ready = ['indexed', 'done', 'completed', 'success', 'processed', 'active'].includes(normalized ?? '')
  const failed = ['error', 'failed', 'failure'].includes(normalized ?? '')
  const working = ['queued', 'parsing', 'processing', 'pending', 'running'].includes(normalized ?? '')
  return <span className={`fox-library-file-status ${failed ? 'is-error' : ready ? 'is-ready' : ''}`} title={status ?? undefined}>
    {ready ? <CheckCircle2 /> : failed ? <CircleAlert /> : working ? <LoaderCircle className="animate-spin" /> : null}
    {ready ? '已索引' : failed ? '处理失败' : working ? '处理中' : normalized === 'stale' ? '待重新处理' : status || '暂无状态'}
  </span>
}

export function LibraryFileManager({ searchQuery, onSearchQueryChange, contentOnly = false, view, files, onOpen, onRemove, onReprocess, onReveal, onDownload, onAdd, onCreateFolder, onDropFiles, onDropPaths, onPathChange, path: controlledPath, busy, footer }: LibraryFileManagerProps) {
  const [internalPath, setInternalPath] = useState('')
  const path = normalizeLibraryPath(controlledPath ?? internalPath)
  const [internalQuery, setInternalQuery] = useState('')
  const query = searchQuery ?? internalQuery
  const setQuery = onSearchQueryChange ?? setInternalQuery
  const [sort, setSort] = useState<'name' | 'time' | 'size'>('name')
  const [descending, setDescending] = useState(false)
  const [selected, setSelected] = useState(new Set<string>())
  const [dragging, setDragging] = useState(false)
  const anchor = useRef<string | null>(null)
  const root = useRef<HTMLElement>(null)
  const selectAllRef = useRef<HTMLInputElement>(null)
  const folderCounts = useMemo(() => libraryFolderCounts(files), [files])
  const folderPaths = useMemo(() => [...folderCounts.keys()].sort((a, b) => a.localeCompare(b, 'zh-CN')), [folderCounts])
  const entries = useMemo<LibraryFile[]>(() => {
    const explicit = new Set(files.filter((file) => file.folder).map((file) => normalizeLibraryPath(file.path)))
    return [...files, ...folderPaths.filter((folder) => !explicit.has(folder)).map((folder) => ({ id: `folder:${folder}`, path: folder, name: folder.split('/').pop() ?? folder, folder: true, size: 0 }))]
  }, [files, folderPaths])
  const byId = useMemo(() => new Map(entries.map((file) => [file.id, file])), [entries])
  const visible = useMemo(() => visibleLibraryFiles(entries, path, query, sort, descending), [entries, path, query, sort, descending])
  const selectable = visible.filter((file) => !file.folder)
  const selectedFiles = files.filter((file) => !file.folder && selected.has(file.id))
  const allSelected = selectable.length > 0 && selectable.every((file) => selected.has(file.id))
  useEffect(() => { if (selectAllRef.current) selectAllRef.current.indeterminate = !allSelected && selectable.some((file) => selected.has(file.id)) }, [allSelected, selectable, selected])
  useEffect(() => { const ids = new Set(files.map((file) => file.id)); setSelected((current) => new Set([...current].filter((id) => ids.has(id)))) }, [files])
  const nativeDrop = useRef({ onDropPaths, path, busy })
  nativeDrop.current = { onDropPaths, path, busy }
  useEffect(() => {
    if (!desktopRuntimeAvailable || !onDropPaths) return
    let disposed = false
    let stop: (() => void) | undefined
    void import('@tauri-apps/api/webview').then(({ getCurrentWebview }) => getCurrentWebview().onDragDropEvent((event) => {
      if (disposed || nativeDrop.current.busy) return
      const payload = event.payload
      if (payload.type === 'leave') { setDragging(false); return }
      const bounds = root.current?.getBoundingClientRect()
      const scale = window.devicePixelRatio || 1
      const x = payload.position.x / scale, y = payload.position.y / scale
      const inside = Boolean(bounds && x >= bounds.left && x <= bounds.right && y >= bounds.top && y <= bounds.bottom)
      setDragging(inside && payload.type !== 'drop')
      if (payload.type === 'drop' && inside) nativeDrop.current.onDropPaths?.(payload.paths, nativeDrop.current.path)
    })).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [Boolean(onDropPaths)])
  useEffect(() => { setQuery(''); setSelected(new Set()); anchor.current = null }, [path])
  const navigate = (next: string) => { const normalized = normalizeLibraryPath(next); setInternalPath(normalized); onPathChange?.(normalized); setQuery(''); setSelected(new Set()); anchor.current = null }
  const toggle = (file: LibraryFile, shift = false) => {
    setSelected((current) => {
      const next = new Set(current)
      const start = selectable.findIndex((entry) => entry.id === anchor.current)
      const end = selectable.findIndex((entry) => entry.id === file.id)
      if (shift && start >= 0 && end >= 0) selectable.slice(Math.min(start, end), Math.max(start, end) + 1).forEach((entry) => next.add(entry.id))
      else if (next.has(file.id)) next.delete(file.id)
      else next.add(file.id)
      return next
    })
    anchor.current = file.id
  }
  const selectAll = () => setSelected((current) => {
    const next = new Set(current)
    selectable.forEach((file) => { if (allSelected) next.delete(file.id); else next.add(file.id) })
    return next
  })
  const changeSort = (value: typeof sort) => { if (sort === value) setDescending((current) => !current); else { setSort(value); setDescending(value !== 'name') } }
  const date = (value?: string | null) => value ? new Date(value).toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' }) : '未记录'
  const corrFiles = useMemo<FileManagerItem[]>(() => entries.map((file) => ({ id: file.id, name: file.name, path: `/${libraryParentPath(file.path)}`, kind: file.folder ? 'folder' : 'file', size: formatBytes(file.size), modified: date(file.modifiedAt) })), [entries])
  const filterFiles = useCallback((items: FileManagerItem[]) => { const map = new Map(items.map((item) => [item.id, item])); return visible.flatMap((file) => { const item = map.get(file.id); return item ? [item] : [] }) }, [visible])
  const fileMenu = (file: LibraryFile) => <DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" aria-label={`${file.name} 的更多操作`}><MoreHorizontal /></Button></DropdownMenuTrigger><DropdownMenuContent className="fox-library-file-menu" align="end">
    <DropdownMenuItem onSelect={() => onOpen(file)}><Eye />打开</DropdownMenuItem>
    {onReprocess && <DropdownMenuItem disabled={busy} onSelect={() => onReprocess([file])}><RotateCcw />重新处理</DropdownMenuItem>}
    {onReveal && <DropdownMenuItem onSelect={() => onReveal(file)}><FolderOpen />在文件夹中显示</DropdownMenuItem>}
    {onDownload && <DropdownMenuItem disabled={busy} onSelect={() => onDownload([file])}><Download />下载原件</DropdownMenuItem>}
    {onRemove && <><DropdownMenuSeparator /><DropdownMenuItem onSelect={() => onRemove([file])}><X />从资料库移除</DropdownMenuItem></>}
  </DropdownMenuContent></DropdownMenu>

  return <section ref={root} className={`fox-library-manager ${contentOnly ? 'is-content-only' : ''} ${dragging ? 'is-dragging' : ''}`} aria-label="资料文件管理器" onKeyDown={(event) => {
    if ((event.target as HTMLElement).matches('input,textarea,[contenteditable="true"]')) return
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'a') { event.preventDefault(); setSelected(new Set(selectable.map((file) => file.id))) }
    if (event.key === 'Escape') setSelected(new Set())
  }} onDragOver={onDropFiles ? (event) => { if (event.dataTransfer.types.includes('Files')) { event.preventDefault(); setDragging(true) } } : undefined} onDragLeave={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node)) setDragging(false) }} onDrop={onDropFiles ? (event) => { event.preventDefault(); setDragging(false); if (!busy) onDropFiles([...event.dataTransfer.files], path) } : undefined}>
    <FileManager contentOnly={contentOnly} view={view} className={contentOnly ? "fox-library-file-content" : "fox-library-corr"} files={corrFiles} path={`/${path}`} onPathChange={navigate} onOpen={(item) => { const file = byId.get(item.id); if (file) onOpen(file) }} filterFiles={filterFiles}
      toolbar={<label className="fox-library-search"><Search /><Input aria-label="搜索当前资料库文件" placeholder="在当前资料库中搜索" value={query} onChange={(event) => setQuery(event.target.value)} /></label>}
      treeFooter={onCreateFolder && <Button variant="ghost" size="sm" onClick={() => onCreateFolder(path)}><FolderPlus />新建文件夹</Button>}
      renderFiles={({ files: rendered, view, onOpen: open }) => <>
        {selectedFiles.length > 0 && <div className="fox-library-bulk" aria-label="已选文件操作">
          <b role="status">已选 {selectedFiles.length} 个</b><span>{formatBytes(selectedFiles.reduce((total, file) => total + file.size, 0))}</span><div className="fox-library-spacer" />
          {onReprocess && <Button size="sm" variant="outline" disabled={busy} onClick={() => onReprocess(selectedFiles)}><RotateCcw />重新处理</Button>}
          {onDownload && <Button size="sm" variant="outline" disabled={busy} onClick={() => onDownload(selectedFiles)}><Download />下载原件</Button>}
          {onRemove && <Button size="sm" variant="outline" disabled={busy} onClick={() => onRemove(selectedFiles)}><X />从资料库移除</Button>}
          <Button variant="ghost" size="icon" aria-label="清除选择" onClick={() => setSelected(new Set())}><X /></Button>
        </div>}
        <div className="fox-library-sortbar">
          <label><input ref={selectAllRef} type="checkbox" checked={allSelected} disabled={!selectable.length} onChange={selectAll} aria-label="选择当前列表中的全部文件" /><span className="sr-only">全选</span></label>
          <button type="button" className="fox-library-col-name" onClick={() => changeSort('name')} aria-label="按名称排序">名称{sort === 'name' && (descending ? <ArrowDown /> : <ArrowUp />)}</button>
          <span className="fox-library-col-type">类型</span>
          <button type="button" className="fox-library-col-size" onClick={() => changeSort('size')} aria-label="按大小排序">大小{sort === 'size' && (descending ? <ArrowDown /> : <ArrowUp />)}</button>
          <span className="fox-library-col-status">状态</span>
          <button type="button" className="fox-library-col-time" onClick={() => changeSort('time')} aria-label="按修改时间排序">修改时间{sort === 'time' && (descending ? <ArrowDown /> : <ArrowUp />)}</button>
          <span className="fox-library-col-actions">操作</span>
        </div>
        <div className={`fox-library-files is-${view === 'cards' ? 'cards' : 'list'} ${view === 'list' ? 'is-compact' : ''}`} role="list" aria-label="文件">
          {rendered.map((item) => {
            const file = byId.get(item.id)
            if (!file) return null
            return <div key={file.id} className={`fox-library-file ${selected.has(file.id) ? 'is-selected' : ''}`} role="listitem" tabIndex={file.folder ? undefined : 0} aria-label={file.folder ? undefined : `${file.name}${selected.has(file.id) ? '，已选中' : ''}`} onClick={file.folder ? undefined : (event) => { if (event.shiftKey || event.ctrlKey || event.metaKey) toggle(file, event.shiftKey); else { setSelected((current) => current.has(file.id) ? new Set([...current].filter((id) => id !== file.id)) : new Set([file.id])); anchor.current = file.id } }} onKeyDown={file.folder ? undefined : (event) => { if (event.target !== event.currentTarget) return; if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); if (event.shiftKey || event.ctrlKey || event.metaKey) toggle(file, event.shiftKey); else { setSelected((current) => current.has(file.id) ? new Set([...current].filter((id) => id !== file.id)) : new Set([file.id])); anchor.current = file.id } } }}>
              {!file.folder ? <input type="checkbox" checked={selected.has(file.id)} onClick={(event) => { event.stopPropagation(); toggle(file, event.shiftKey) }} onChange={() => {}} aria-label={`选择 ${file.name}`} /> : <span className="fox-library-checkbox-space" />}
              {file.folder ? <button type="button" className="fox-library-file-open" onClick={() => open(item)} title={file.path}><Folder className="fox-library-folder-icon" /><span><b>{file.name}</b></span></button> : <span className="fox-library-file-open" title={file.path}><LocalFileTypeIcon extension={file.name.split('.').pop() ?? ''} /><span><b>{file.name}</b>{query && <small>{file.path}</small>}</span></span>}
              <span className="fox-library-file-type">{file.folder ? '文件夹' : file.name.split('.').pop()?.toUpperCase()}</span>
              <span className="fox-library-file-size">{file.folder ? `${folderCounts.get(normalizeLibraryPath(file.path)) ?? 0} 项` : formatBytes(file.size)}</span>
              <span className="fox-library-file-processing">{!file.folder && <ProcessingState status={file.status} />}</span>
              <time className="fox-library-file-time">{file.folder ? '' : date(file.modifiedAt)}</time>
              {!file.folder && <span className="fox-library-row-actions" onClick={(event) => event.stopPropagation()} onKeyDown={(event) => event.stopPropagation()}><Button variant="ghost" size="icon" aria-label={`预览 ${file.name}`} title="预览" onClick={() => onOpen(file)}><Eye /></Button>{fileMenu(file)}</span>}
            </div>
          })}
          {!rendered.length && <div className="fox-library-empty"><FolderOpen /><b>{query ? '没有找到匹配的文件' : '这个文件夹还是空的'}</b><p>{query ? '试试文件名或路径中的其他关键词。' : onAdd ? '从电脑里挑选文件，或把文件拖到这里。' : '选择其他目录，或稍后刷新资料库。'}</p>{onAdd && <Button variant="outline" onClick={() => onAdd(path)}><Plus />添加文件</Button>}</div>}
        </div>
        {onAdd && rendered.length > 0 && <button type="button" className="fox-library-drop-hint" onClick={() => onAdd(path)}><Upload />拖入文件，或点这里添加</button>}
        {!contentOnly && <footer className="fox-library-file-footer" role="status">{rendered.length} 个项目{selectedFiles.length > 0 && `，已选 ${selectedFiles.length} 个`}{footer}</footer>}
      </>} />
    {dragging && <div className="fox-library-drop-overlay" role="status"><Upload /><b>松开以添加文件</b><span>导入到 {path || '全部资料'}</span></div>}
  </section>
}

export function LibraryViewSwitch({ view, onChange }: { view: FileManagerView; onChange: (view: FileManagerView) => void }) {
  return <div className="fox-library-view-switch" role="group" aria-label="文件视图">{([{ value: 'details', label: '详细列表', icon: Rows3 }, { value: 'list', label: '紧凑列表', icon: List }, { value: 'cards', label: '网格视图', icon: Grid2X2 }] as const).map(({ value, label, icon: Icon }) => <Button key={value} variant="ghost" size="icon" title={label} aria-label={label} aria-pressed={view === value} onClick={() => onChange(value)}><span className="fox-library-view-indicator"><Icon /></span></Button>)}</div>
}
