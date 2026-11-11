import { useCallback, useEffect, useRef, useState } from 'react'
import { FolderOpen, MoreHorizontal } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { desktopClient, desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import type { ConversationDetail } from '@/features/conversations/model/types'
import { contacts, type Companion, type CompanionFileEntry, type CompanionFiles } from './api'
import { ContactAvatar } from './chat-controls'
import { formatSearchTime } from './chat-search'
import { showContactError, useContacts } from './context'
import {
  LIBRARY_GROUPS,
  LibraryGlyph,
  formatLibraryRelative,
  libraryGroupLabel,
  libraryMediaKind,
  libraryTimeGroup,
  type LibraryTimeGroup,
} from './library-shared'
import { managedFileGroupAt, managedFilePathKey, type ManagedFileGroup } from '@/features/chat/components/managed-files/model'
import {
  ManagedFileRow,
  ManagedFilesFootnote,
  ManagedFilesStatus,
  VersionDriftWarn,
  VersionRows,
  VersionToggle,
} from '@/features/chat/components/managed-files/ManagedFileVersions'
import type { ManagedFilesController } from '@/features/chat/components/managed-files/use-managed-files'
import './contacts.css'

// The row helpers (media kind, time bands, relative time) live in
// `library-shared.tsx` so the chat surfaces can reuse them without re-entering
// this module; re-exported here because callers and tests address them here.
export { formatLibraryRelative, libraryMediaKind, libraryTimeGroup } from './library-shared'
export type { LibraryTimeGroup } from './library-shared'

export const FILES_WORKDIR_HINT = '手工放入工作目录、没有消息记录的文件，请用「打开工作目录」查看。'

const COLLAPSED_ROWS = 5
type LibraryFilter = 'all' | 'media' | 'files'
type LibraryDetail = Pick<ConversationDetail, 'attachments' | 'artifacts'>
const FILTERS: Array<[LibraryFilter, string]> = [['all', '全部'], ['media', '媒体'], ['files', '文件']]

/** One card row: a message-recorded file, optionally carrying its managed
 *  version group, or a managed-only file with no message record (orphan). */
type LibraryRow = {
  key: string
  at: number
  entry: CompanionFileEntry | null
  group: ManagedFileGroup | null
}

function groupLibraryRows(data: CompanionFiles | null, detail: LibraryDetail | null | undefined, managedGroups: ManagedFileGroup[], filter: LibraryFilter, now: number) {
  const paths = new Map<string, string>()
  for (const attachment of detail?.attachments ?? []) paths.set(`attachment:${attachment.id}`, attachment.storagePath)
  for (const artifact of detail?.artifacts ?? []) paths.set(`artifact:${artifact.id}`, artifact.storagePath)
  const groupByPath = new Map<string, ManagedFileGroup>()
  for (const group of managedGroups) {
    const path = managedFilePathKey(group.storagePath)
    if (!groupByPath.has(path)) groupByPath.set(path, group)
  }
  const rows: LibraryRow[] = []
  const matched = new Set<ManagedFileGroup>()
  for (const entry of [...(data?.provided ?? []), ...(data?.generated ?? [])]) {
    if (filter !== 'all' && (filter === 'media') !== Boolean(libraryMediaKind(entry))) continue
    const key = `${entry.kind}:${entry.id}`
    const path = paths.get(key)
    const group = path ? groupByPath.get(managedFilePathKey(path)) : undefined
    rows.push({ key, at: entry.updatedAt, entry, group: group && !matched.has(group) ? group : null })
    if (group) matched.add(group)
  }
  for (const group of managedGroups) {
    if (matched.has(group)) continue
    if (filter !== 'all' && (filter === 'media') !== Boolean(libraryMediaKind({ name: group.displayName }))) continue
    rows.push({ key: `managed:${group.storagePath}`, at: managedFileGroupAt(group), entry: null, group })
  }
  rows.sort((a, b) => b.at - a.at)
  return LIBRARY_GROUPS
    .map(([value, label]) => ({ value, label, rows: rows.filter(row => libraryTimeGroup(row.at, now) === value) }))
    .filter(group => group.rows.length > 0)
}

function FileRow({ contact, entry, group: timeGroup, now, versions }: {
  contact: Companion
  entry: CompanionFileEntry
  group: LibraryTimeGroup
  now: number
  versions: { group: ManagedFileGroup; controller: ManagedFilesController } | null
}) {
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
  const open = versions ? Boolean(versions.controller.expanded[versions.group.storagePath]) : false
  return <>
    <div className={`fox-library-row${open ? ' is-open' : ''}`} ref={rowRef} aria-label={entry.name} onMouseEnter={place} onFocus={place}>
      <LibraryGlyph entry={entry} />
      <span className="fox-library-meta">
        <span className="fox-library-name">{entry.name}</span>
        {versions && <VersionDriftWarn group={versions.group} />}
      </span>
      {versions && <VersionToggle
        count={versions.group.versions.length}
        open={open}
        onToggle={() => versions.controller.toggleExpanded(versions.group.storagePath)}
      />}
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
        <span className="fox-library-hover-group">{libraryGroupLabel(timeGroup)}</span>
        <span className="fox-library-hover-kind">{provenance}{entry.missing && <em> · 文件缺失</em>}</span>
        <b className="fox-library-hover-name">{entry.name}</b>
        <span className="fox-library-hover-source"><ContactAvatar contact={contact} /><span>{contact.displayName} · {hoverSource}</span></span>
      </div>
    </div>
    {versions && open && <VersionRows group={versions.group} controller={versions.controller} />}
  </>
}

export function ContactFiles({ contact, onBack, managed, detail }: {
  contact: Companion
  onBack?: () => void
  /** Versions of Host-managed files for this conversation, owned by the parent
   *  surface (it also drives the unseen dot). Absent = no version UI. */
  managed?: ManagedFilesController | null
  detail?: LibraryDetail | null
}) {
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
  const managedGroups = managed?.state.status === 'ready' ? managed.state.groups : []
  const groups = groupLibraryRows(data, detail, managedGroups, filter, now)
  return <div className="fox-library" aria-busy={busy}>
    {onBack && <div className="fox-contact-capabilities-head"><h3>资料库</h3>
      <Button size="sm" variant="ghost" onClick={onBack}>返回资料</Button></div>}
    {error && <p role="alert">{error} <button type="button" onClick={() => void reload()}>重试</button></p>}
    {!data && !error && <p className="fox-library-loading" role="status">正在读取资料库…</p>}
    {(data || managedGroups.length > 0) &&
      <div className="fox-library-filters" role="group" aria-label="资料筛选">
        {FILTERS.map(([value, label]) => <button key={value} type="button" aria-pressed={filter === value}
          className={filter === value ? 'is-selected' : undefined} onClick={() => setFilter(value)}>{label}</button>)}
      </div>}
    {groups.map(group => <section className="fox-library-group" key={group.value} aria-label={group.label}>
      <h3>{group.label}</h3>
      <div className="fox-library-card">
        {(expanded[group.value] ? group.rows : group.rows.slice(0, COLLAPSED_ROWS)).map(row => row.entry
          ? <FileRow key={row.key} contact={contact} entry={row.entry} group={group.value} now={now}
              versions={row.group && managed ? { group: row.group, controller: managed } : null} />
          : row.group && managed
            ? <ManagedFileRow key={row.key} group={row.group} controller={managed} />
            : null)}
        {group.rows.length > COLLAPSED_ROWS && <button type="button" className="fox-library-more" aria-expanded={expanded[group.value]}
          onClick={() => setExpanded(previous => ({ ...previous, [group.value]: !previous[group.value] }))}>{expanded[group.value] ? '收起' : '展开'}</button>}
      </div>
    </section>)}
    {data && managed?.state.status !== 'loading' && !groups.length && <p className="fox-library-empty">{filter === 'all' ? '资料库还没有文件。' : '当前筛选下没有文件。'}</p>}
    {data && !data.provided.length && <p className="fox-library-hint">还没有通过消息提供过文件。</p>}
    {data && !data.generated.length && <p className="fox-library-hint">还没有生成过文件。</p>}
    {managed && managedGroups.length > 0 && <ManagedFilesFootnote controller={managed} />}
    {managed && <ManagedFilesStatus controller={managed} />}
    <p className="fox-library-hint">{FILES_WORKDIR_HINT}</p>
    <div className="fox-library-foot"><div className="fox-library-card"><button type="button" className="fox-library-action" disabled={busy} onClick={() => void openWorkspace()}>
      <FolderOpen size={16} aria-hidden="true" /><span>打开工作目录</span>
    </button></div></div>
  </div>
}
