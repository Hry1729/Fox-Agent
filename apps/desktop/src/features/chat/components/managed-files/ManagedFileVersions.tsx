// Shared managed-file version UI, rendered with the 资料库 row language on both
// surfaces: the file cards (via `VersionToggle` / `VersionRows` composed inside
// `files.tsx`) and the standalone time-grouped list above the project tree in
// the right sidebar 文件 page (`ManagedFileVersions`). One restore rule lives
// in the store; these components only view it.
import { Fragment } from 'react'
import { AlertTriangle, ChevronDown, ChevronUp, LoaderCircle, RefreshCw, RotateCcw } from 'lucide-react'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { LibraryGlyph, LIBRARY_GROUPS, libraryTimeGroup } from '@/features/companions/library-shared'
import { managedFileVersionPresentation } from '../managed-file-presentation'
import {
  KIND_COPY,
  formatVersionBytes,
  formatVersionTime,
  managedFileGroupAt,
  type ManagedFileGroup,
} from './model'
import type { ManagedFilesController } from './use-managed-files'
import './managed-files.css'

export const MANAGED_FILES_BACKUP_NOTE = '恢复前会自动备份当前内容'
/** The ⓘ tooltip: what the registry does NOT cover. Kept from the old panel. */
export const MANAGED_FILES_SCOPE_NOTE = '注意：run_command 内任意命令、通用 MCP/远程服务造成的文件或外部副作用不在版本记录内，恢复不会重放任何工具调用。'

export function VersionToggle({ count, open, onToggle }: { count: number; open: boolean; onToggle: () => void }) {
  return (
    <button type="button" className="fox-version-toggle" aria-expanded={open} title="查看文件版本" onClick={onToggle}>
      <span>{count} 个版本</span>
      {open ? <ChevronUp size={12} aria-hidden="true" /> : <ChevronDown size={12} aria-hidden="true" />}
    </button>
  )
}

export function VersionDriftWarn({ group }: { group: ManagedFileGroup }) {
  if (!group.drifted) return null
  return (
    <span className={`fox-version-warn${group.deleted ? ' is-deleted' : ''}`}>
      <AlertTriangle size={11} aria-hidden="true" />
      {group.deleted ? '文件已被删除' : '之后被外部修改'}
    </span>
  )
}

export function VersionRows({ group, controller }: { group: ManagedFileGroup; controller: ManagedFilesController }) {
  return (
    <div className="fox-versions">
      {group.versions.map((version, index) => {
        // One rule, shared with the tests: the row's own content size, and
        // restore offered exactly when that content resolves.
        const view = managedFileVersionPresentation(version, { isLatest: index === 0, drifted: group.drifted })
        const hint = controller.forceHint[version.id]
        const busy = controller.busyId === version.id
        return (
          <Fragment key={version.id}>
            <div className="fox-version">
              <span className="fox-version-tag">v{version.versionNo}</span>
              <span className="fox-version-kind" title={`${version.tool} · ${formatVersionBytes(view.sizeLabel)} · ${new Date(version.createdAt).toLocaleString('zh-CN')}`}>
                {KIND_COPY[version.changeKind] ?? version.changeKind} · {formatVersionTime(version.createdAt)}
                {version.changeKind === 'restored' && version.restoredFromId && <em className="fox-version-source">旧版恢复</em>}
              </span>
              {view.current
                ? <span className="fox-version-current">当前</span>
                : <button
                    type="button"
                    className="fox-version-restore"
                    disabled={controller.busyId !== null || !view.restorable}
                    title={view.restorable ? '把文件恢复为该版本记录的内容（当前内容会先备份）' : view.blocker ?? '该版本没有可验证的内容快照，无法恢复'}
                    onClick={() => controller.restore(version, !!hint)}
                  >
                    {busy ? <LoaderCircle size={11} className="animate-spin" aria-hidden="true" /> : <RotateCcw size={11} aria-hidden="true" />}
                    {hint ? '覆盖恢复' : '恢复'}
                  </button>}
            </div>
            {!view.restorable && !view.current && view.blocker && <p className="fox-version-note">{view.blocker}</p>}
            {hint && (
              <p className="fox-version-note is-force">
                {hint}
                <button type="button" disabled={controller.busyId !== null} onClick={() => controller.restore(version, true)}>强制恢复（先备份当前内容）</button>
              </p>
            )}
          </Fragment>
        )
      })}
    </div>
  )
}

/** One managed file as a library row: glyph, name (+ drift warning), toggle.
 *  Used standalone by the right sidebar and for managed files that have no
 *  message record inside the 资料库. */
export function ManagedFileRow({ group, controller }: { group: ManagedFileGroup; controller: ManagedFilesController }) {
  const open = Boolean(controller.expanded[group.storagePath])
  return (
    <>
      <div className={`fox-library-row fox-managed-row${open ? ' is-open' : ''}`} title={group.storagePath}>
        <LibraryGlyph entry={{ name: group.displayName }} />
        <span className="fox-library-meta">
          <span className="fox-library-name">{group.displayName}</span>
          <VersionDriftWarn group={group} />
        </span>
        <VersionToggle count={group.versions.length} open={open} onToggle={() => controller.toggleExpanded(group.storagePath)} />
      </div>
      {open && <VersionRows group={group} controller={controller} />}
    </>
  )
}

/** Backup reminder + scope tooltip + manual re-check, shown under both
 *  surfaces' version lists. */
export function ManagedFilesFootnote({ controller }: { controller: ManagedFilesController }) {
  return (
    <p className="fox-version-hint">
      <span>{MANAGED_FILES_BACKUP_NOTE}</span>
      <TooltipProvider>
        <Tooltip>
          <TooltipTrigger asChild><button type="button" className="fox-version-info" aria-label="版本记录范围">i</button></TooltipTrigger>
          <TooltipContent>{MANAGED_FILES_SCOPE_NOTE}</TooltipContent>
        </Tooltip>
      </TooltipProvider>
      <button type="button" className="fox-version-refresh" title="重新检查文件版本" aria-label="重新检查文件版本" onClick={controller.refresh}>
        {controller.state.status === 'loading' ? <LoaderCircle size={11} className="animate-spin" /> : <RefreshCw size={11} />}
      </button>
    </p>
  )
}

export function ManagedFilesStatus({ controller }: { controller: ManagedFilesController }) {
  const { state, restoreError } = controller
  const readError = state.status === 'error' ? state.message : state.status === 'ready' ? state.error : null
  return <>
    {state.status === 'loading' && <p className="fox-version-state" role="status"><LoaderCircle size={12} className="animate-spin" />正在读取文件版本…</p>}
    {readError && <p className="fox-version-state is-error" role="alert">
      {readError}<button type="button" onClick={controller.refresh}>重试</button>
    </p>}
    {restoreError && <p className="fox-version-state is-error" role="alert">
      {restoreError.message}
      <button type="button" disabled={controller.busyId !== null}
        onClick={() => controller.restore(restoreError.version, restoreError.force)}>重试恢复</button>
    </p>}
  </>
}

/**
 * The version history as its own list — the right sidebar 文件 page renders it
 * above the project tree with the 资料库 time groups and file rows. Contact
 * cards embed `VersionToggle` / `VersionRows` in their own rows instead.
 */
export function ManagedFileVersions({ controller }: { controller: ManagedFilesController }) {
  const groups = controller.state.status === 'ready' ? controller.state.groups : []
  if (groups.length === 0) return <ManagedFilesStatus controller={controller} />
  const now = Date.now()
  const bands = LIBRARY_GROUPS
    .map(([value, label]) => ({ value, label, items: groups.filter(group => libraryTimeGroup(managedFileGroupAt(group), now) === value) }))
    .filter(band => band.items.length > 0)
  return (
    <div className="fox-file-versions">
      {bands.map(band => (
        <section className="fox-library-group" key={band.value} aria-label={band.label}>
          <h3>{band.label}</h3>
          <div className="fox-library-card">
            {band.items.map(group => <ManagedFileRow key={group.storagePath} group={group} controller={controller} />)}
          </div>
        </section>
      ))}
      <ManagedFilesFootnote controller={controller} />
      <ManagedFilesStatus controller={controller} />
    </div>
  )
}
