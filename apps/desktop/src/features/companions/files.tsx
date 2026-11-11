import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { FileAudio, FileVideo, FolderOpen, Image as ImageIcon, MoreHorizontal } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { desktopClient, desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import { contacts, type Companion, type CompanionFileEntry, type CompanionFiles } from './api'
import { ContactAvatar } from './chat-controls'
import { formatSearchTime } from './chat-search'
import { showContactError, useContacts } from './context'
import './contacts.css'

export const FILES_WORKDIR_HINT = '手工放入工作目录、没有消息记录的文件，请用「打开工作目录」查看。'

const COLLAPSED_ROWS = 5
type LibraryFilter = 'all' | 'media' | 'files'
const FILTERS: Array<[LibraryFilter, string]> = [['all', '全部'], ['media', '媒体'], ['files', '文件']]
export type LibraryTimeGroup = 'today' | 'week' | 'earlier'
const GROUPS: Array<[LibraryTimeGroup, string]> = [['today', '今天'], ['week', '本周'], ['earlier', '更早']]

export function libraryMediaKind(entry: CompanionFileEntry): 'image' | 'video' | 'audio' | null {
  const mime = entry.mediaType ?? ''
  if (mime.startsWith('image/')) return 'image'
  if (mime.startsWith('video/')) return 'video'
  if (mime.startsWith('audio/')) return 'audio'
  const ext = entry.name.includes('.') ? entry.name.split('.').pop()!.toLowerCase() : ''
  if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp', 'avif'].includes(ext)) return 'image'
  if (['mp4', 'mov', 'webm', 'mkv', 'avi'].includes(ext)) return 'video'
  if (['mp3', 'wav', 'ogg', 'm4a', 'flac'].includes(ext)) return 'audio'
  return null
}

export function libraryTimeGroup(at: number, now: number): LibraryTimeGroup {
  const date = new Date(at)
  const current = new Date(now)
  if (date.toDateString() === current.toDateString()) return 'today'
  const weekStart = new Date(current)
  weekStart.setHours(0, 0, 0, 0)
  weekStart.setDate(current.getDate() - ((current.getDay() + 6) % 7))
  return at >= weekStart.getTime() ? 'week' : 'earlier'
}

export function formatLibraryRelative(at: number, now = Date.now()): string {
  const minutes = Math.floor(Math.max(0, now - at) / 60_000)
  if (minutes < 1) return '刚刚'
  if (minutes < 60) return `${minutes} 分钟`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} 小时`
  const days = Math.floor(hours / 24)
  if (days < 7) return `${days} 天`
  return formatSearchTime(at, now)
}

const BAND_COLORS: Record<string, string> = {
  pdf: '#e5484d', doc: '#3e63dd', docx: '#3e63dd', xls: '#30a46c', xlsx: '#30a46c', csv: '#30a46c',
  ppt: '#f76b15', pptx: '#f76b15', zip: '#f5a524', rar: '#f5a524', '7z': '#f5a524',
}

function FileGlyph({ entry }: { entry: CompanionFileEntry }) {
  const media = libraryMediaKind(entry)
  const missing = entry.missing ? ' is-missing' : ''
  if (media) {
    const Icon = media === 'image' ? ImageIcon : media === 'video' ? FileVideo : FileAudio
    return <span className={`fox-library-glyph is-media is-${media}${missing}`} aria-hidden="true"><Icon size={15} /></span>
  }
  const ext = entry.name.includes('.') ? entry.name.split('.').pop()!.toLowerCase() : ''
  const band = BAND_COLORS[ext]
  return <svg className={`fox-library-glyph is-doc${missing}`} viewBox="0 0 20 20" aria-hidden="true">
    <path className="fox-library-page" d="M4.5 1.5h7.5l3.5 3.5v13.5h-11z" />
    <path className="fox-library-fold" d="M12 1.5V5h3.5" />
    {band
      ? <rect x="2.5" y="10.5" width="15" height="4.5" rx="1.5" fill={band} />
      : <path className="fox-library-line" d="M7 8.5h6M7 11.5h6M7 14.5h4" />}
  </svg>
}

function FileRow({ contact, entry, group, now }: { contact: Companion; entry: CompanionFileEntry; group: LibraryTimeGroup; now: number }) {
  const ctx = useContacts()
  const [busy, setBusy] = useState(false)
  const [hoverSide, setHoverSide] = useState<'above' | 'below'>('above')
  const [menuUp, setMenuUp] = useState(false)
  const rowRef = useRef<HTMLDivElement>(null)
  const menuRef = useRef<HTMLDetailsElement>(null)
  const act = async (action: () => Promise<unknown>) => {
    setBusy(true)
    try { await action() } catch (e) { showContactError(e) } finally { setBusy(false) }
  }
  // Both floaters flip when the row sits near the scrollport edge, so the
  // preview and the menu never open into the clipped area.
  const place = () => {
    const row = rowRef.current
    const scroller = row?.closest('.fox-contact-side-card-body, .fox-contact-panel')
    if (!row || !scroller) return
    const rowBox = row.getBoundingClientRect()
    const scrollBox = scroller.getBoundingClientRect()
    setHoverSide(rowBox.top - scrollBox.top < 150 ? 'below' : 'above')
    setMenuUp(scrollBox.bottom - rowBox.bottom < 190)
  }
  useEffect(() => {
    const menu = menuRef.current
    if (!menu) return
    const onPointerDown = (event: PointerEvent) => { if (menu.open && !menu.contains(event.target as Node)) menu.open = false }
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === 'Escape') menu.open = false }
    document.addEventListener('pointerdown', onPointerDown)
    document.addEventListener('keydown', onKeyDown)
    return () => { document.removeEventListener('pointerdown', onPointerDown); document.removeEventListener('keydown', onKeyDown) }
  }, [])
  const closeMenu = () => { if (menuRef.current) menuRef.current.open = false }
  const provenance = entry.kind === 'attachment' ? '我提供的文件' : '联系人生成的结果'
  const sourceLine = `${entry.origin === 'task' ? `定时任务「${entry.taskName ?? '未命名'}」 · ` : ''}${entry.source ? `来源消息 · ${formatSearchTime(entry.source.at, now)}` : '暂无来源消息'}`
  const hoverSource = `${entry.origin === 'task' ? `定时任务「${entry.taskName ?? '未命名'}」 · ` : ''}${entry.source ? formatSearchTime(entry.source.at, now) : '暂无来源消息'}`
  return <div className="fox-library-row" ref={rowRef} aria-label={entry.name} onMouseEnter={place} onFocus={place}>
    <FileGlyph entry={entry} />
    <span className="fox-library-name">{entry.name}</span>
    {entry.missing
      ? <span className="fox-library-missing" role="status">文件缺失</span>
      : <time className="fox-library-time" dateTime={new Date(entry.updatedAt).toISOString()}>{formatLibraryRelative(entry.updatedAt, now)}</time>}
    <details className="fox-library-menu" ref={menuRef} onToggle={place}>
      <summary aria-label={`${entry.name} 的操作`} title="更多操作"><MoreHorizontal size={16} aria-hidden="true" /></summary>
      <div className={`fox-library-menu-pop${menuUp ? ' is-up' : ''}`}>
        <p className="fox-library-menu-head">{provenance} · {sourceLine}</p>
        <button type="button" disabled={busy || entry.missing} onClick={() => { closeMenu(); void act(() => contacts.openFile(contact.id, entry.kind, entry.id)) }}>打开</button>
        <button type="button" disabled={busy || entry.missing} onClick={() => { closeMenu(); void act(() => contacts.saveFile(contact.id, entry.kind, entry.id)) }}>另存为</button>
        <button type="button" disabled={busy || !entry.source} onClick={() => { closeMenu(); if (entry.source) ctx?.jumpToMessage?.(entry.source.bubbleId) }}>回到消息</button>
      </div>
    </details>
    <div className={`fox-library-hover is-${hoverSide}`} aria-hidden="true">
      <span className="fox-library-hover-group">{GROUPS.find(([value]) => value === group)![1]}</span>
      <span className="fox-library-hover-kind">{provenance}{entry.missing && <em> · 文件缺失</em>}</span>
      <b className="fox-library-hover-name">{entry.name}</b>
      <span className="fox-library-hover-source"><ContactAvatar contact={contact} /><span>{contact.displayName} · {hoverSource}</span></span>
    </div>
  </div>
}

export function ContactFiles({ contact, onBack }: { contact: Companion; onBack?: () => void }) {
  const [data, setData] = useState<CompanionFiles | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [filter, setFilter] = useState<LibraryFilter>('all')
  const [expanded, setExpanded] = useState<Record<LibraryTimeGroup, boolean>>({ today: false, week: false, earlier: false })
  const reload = useCallback(async () => {
    try { setData(await contacts.files(contact.id)); setError('') } catch (e) { setError(desktopErrorDetails(e).message) }
  }, [contact.id])
  useEffect(() => { void reload() }, [reload])
  const openWorkspace = async () => {
    setBusy(true)
    try { await desktopClient.openProjectFolder(contact.conversationId) } catch (e) { showContactError(e) } finally { setBusy(false) }
  }
  const now = Date.now()
  const groups = useMemo(() => {
    if (!data) return []
    const entries = [...data.provided, ...data.generated]
      .filter(entry => filter === 'all' || (filter === 'media') === Boolean(libraryMediaKind(entry)))
      .sort((a, b) => b.updatedAt - a.updatedAt)
    return GROUPS
      .map(([value, label]) => ({ value, label, entries: entries.filter(entry => libraryTimeGroup(entry.updatedAt, now) === value) }))
      .filter(group => group.entries.length > 0)
  }, [data, filter, now])
  return <div className="fox-library" aria-busy={busy}>
    {onBack && <div className="fox-contact-capabilities-head"><h3>资料库</h3>
      <Button size="sm" variant="ghost" onClick={onBack}>返回资料</Button></div>}
    {error && <p role="alert">{error} <button type="button" onClick={() => void reload()}>重试</button></p>}
    {!data ? <p className="fox-library-loading">正在读取资料库…</p> : <>
      <div className="fox-library-filters" role="group" aria-label="资料筛选">
        {FILTERS.map(([value, label]) => <button key={value} type="button" aria-pressed={filter === value}
          className={filter === value ? 'is-selected' : undefined} onClick={() => setFilter(value)}>{label}</button>)}
      </div>
      {groups.map(group => <section className="fox-library-group" key={group.value} aria-label={group.label}>
        <h3>{group.label}</h3>
        <div className="fox-library-card">
          {(expanded[group.value] ? group.entries : group.entries.slice(0, COLLAPSED_ROWS)).map(entry =>
            <FileRow key={entry.id} contact={contact} entry={entry} group={group.value} now={now} />)}
          {group.entries.length > COLLAPSED_ROWS && <button type="button" className="fox-library-more" aria-expanded={expanded[group.value]}
            onClick={() => setExpanded(previous => ({ ...previous, [group.value]: !previous[group.value] }))}>{expanded[group.value] ? '收起' : '展开'}</button>}
        </div>
      </section>)}
      {!groups.length && <p className="fox-library-empty">{filter === 'all' ? '资料库还没有文件。' : '当前筛选下没有文件。'}</p>}
      {!data.provided.length && <p className="fox-library-hint">还没有通过消息提供过文件。</p>}
      {!data.generated.length && <p className="fox-library-hint">还没有生成过文件。</p>}
      <p className="fox-library-hint">{FILES_WORKDIR_HINT}</p>
      <div className="fox-library-foot"><Button size="sm" variant="outline" disabled={busy} onClick={() => void openWorkspace()}><FolderOpen size={14} />打开工作目录</Button></div>
    </>}
  </div>
}