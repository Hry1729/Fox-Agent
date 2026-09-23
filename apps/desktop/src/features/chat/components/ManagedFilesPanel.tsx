// Host-managed file versions ("文件版本恢复"). Every row is appended by the
// Rust Host around an intercepted write (write_file/edit_file/fox-office).
// Restore is a pure Host file copy against a registered backup: it never
// replays tools, never creates a Run, and a later external change blocks it
// unless the user explicitly forces it (the current bytes are backed up
// first). This panel is a thin view over the two read/restore commands; the
// Host owns hashes, drift detection and the append-only registry.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  AlertTriangle,
  ChevronDown,
  FileClock,
  LoaderCircle,
  RefreshCw,
  RotateCcw,
} from 'lucide-react'
import { Button } from '@/components/ui/button'
import { notify as toast } from '@/features/notifications'
import {
  desktopClient,
  desktopRuntimeAvailable,
  desktopErrorDetails,
  type ManagedFileVersion,
} from '@/features/conversations/api/desktop-client'
import { managedFileVersionPresentation } from './managed-file-presentation'
import { RestoreRequestIdentities, restoreActionKey } from './restore-request-identity'

const KIND_COPY: Record<string, string> = {
  created: '新建',
  modified: '修改',
  restored: '恢复',
  // Content a restore overwrote that the registry did not know about (an edit
  // made outside any recorded task). It is preserved as its own version, so the
  // user can select and bring it back.
  replaced: '被覆盖内容',
}

function formatBytes(value: number | null) {
  if (value == null) return ''
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  return `${(value / (1024 * 1024)).toFixed(2)} MB`
}

function formatTime(ms: number) {
  return new Date(ms).toLocaleString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}

interface FileGroup {
  storagePath: string
  displayName: string
  drifted: boolean
  deleted: boolean
  versions: ManagedFileVersion[] // newest version_no first
}

export function ManagedFilesPanel({ conversationId }: { conversationId?: string }) {
  const [rows, setRows] = useState<ManagedFileVersion[]>([])
  const [open, setOpen] = useState(false)
  const [loading, setLoading] = useState(false)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [forceHint, setForceHint] = useState<Record<string, string>>({})
  const conversationIdRef = useRef<string | undefined>(conversationId)
  // Identities of restore actions that are still UNDETERMINED, keyed by the
  // action itself. While a request is in flight (or has failed without a definite
  // end) the same identity is reused, so a double click or a timeout resend is
  // deduplicated by the Host. Once the request reaches a definite end — success,
  // or a rejection the user has seen and answered — the entry is released and
  // the NEXT confirmation mints a new identity.
  const restoreIdentities = useRef<RestoreRequestIdentities>(new RestoreRequestIdentities())
  conversationIdRef.current = conversationId

  const load = useCallback(async (silent = false) => {
    const id = conversationIdRef.current
    if (!desktopRuntimeAvailable || !id) {
      setRows([])
      return
    }
    if (!silent) setLoading(true)
    try {
      const versions = await desktopClient.listManagedFileVersions(id)
      setRows(versions)
      setError(null)
    } catch (cause) {
      if (!silent) setError(desktopErrorDetails(cause).message)
    } finally {
      if (!silent) setLoading(false)
    }
  }, [])

  useEffect(() => {
    setRows([])
    setError(null)
    setForceHint({})
    void load()
  }, [conversationId, load])

  // Versions are appended at dispatch-settle boundaries; reload when the
  // Kernel invalidates its state so the list stays current during a Run.
  useEffect(() => {
    if (!desktopRuntimeAvailable || !conversationId) return
    let unlisten: (() => void) | undefined
    void desktopClient.listenKernelStateInvalidations(() => void load(true)).then((fn) => {
      unlisten = fn
    })
    return () => unlisten?.()
  }, [conversationId, load])

  const groups = useMemo<FileGroup[]>(() => {
    const map = new Map<string, FileGroup>()
    for (const row of rows) {
      let group = map.get(row.storagePath)
      if (!group) {
        group = {
          storagePath: row.storagePath,
          displayName: row.displayName,
          drifted: false,
          deleted: false,
          versions: [],
        }
        map.set(row.storagePath, group)
      }
      group.versions.push(row)
    }
    for (const group of map.values()) {
      group.versions.sort((a, b) => b.versionNo - a.versionNo)
      const latest = group.versions[0]
      group.drifted = latest?.drifted ?? false
      group.deleted = latest?.drifted === true && latest.currentHash == null
    }
    return [...map.values()].sort(
      (a, b) => (b.versions[0]?.createdAt ?? 0) - (a.versions[0]?.createdAt ?? 0),
    )
  }, [rows])

  const restore = useCallback(
    async (version: ManagedFileVersion, force: boolean) => {
      const id = conversationIdRef.current
      if (!id || busyId) return
      if (
        force &&
        !window.confirm(
          `文件 ${version.displayName} 在任务之外被修改过。\n强制恢复会先把当前内容备份，再覆盖为所选版本。是否继续？`,
        )
      ) {
        return
      }
      setBusyId(version.id)
      setError(null)
      try {
        // The action is "restore THIS version, with or without force". Whether
        // the same version was restored before is irrelevant: each confirmation
        // is a new action once the previous one has ended.
        const actionKey = restoreActionKey(version.id, force)
        const requestId = restoreIdentities.current.acquire(
          actionKey,
          () =>
            globalThis.crypto?.randomUUID?.() ??
            `restore-${Date.now()}-${Math.random().toString(16).slice(2)}`,
        )
        const restored = await desktopClient.restoreManagedFileVersion(
          id,
          version.id,
          force,
          requestId,
        )
        // Definite end: release the identity so the next confirmation is a new
        // action rather than a resend of this one.
        restoreIdentities.current.release(actionKey)
        setForceHint((value) => {
          const next = { ...value }
          delete next[version.id]
          return next
        })
        await load(true)
        toast.success(
          `已恢复 ${version.displayName} 到 v${restored.versionNo}，当前内容已自动备份`,
        )
      } catch (cause) {
        const details = desktopErrorDetails(cause)
        setError(details.message)
        // Drift/deletion rejections are exactly the cases force handles.
        if (!force) {
          setForceHint((value) => ({ ...value, [version.id]: details.message }))
        }
      } finally {
        setBusyId(null)
      }
    },
    [busyId, load],
  )

  if (!desktopRuntimeAvailable || !conversationId || rows.length === 0) return null

  return (
    <div className="fox-managed-panel">
      <button
        type="button"
        className="fox-managed-header"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <FileClock size={13} />
        <span>文件版本恢复</span>
        <span className="fox-managed-count">{groups.length}</span>
        {groups.some((group) => group.drifted) && (
          <AlertTriangle size={12} className="fox-managed-warn-icon" aria-label="有文件被外部修改" />
        )}
        <ChevronDown size={13} className={`fox-managed-chevron ${open ? 'is-open' : ''}`} />
      </button>
      {open && (
        <div className="fox-managed-body">
          <div className="fox-managed-toolbar">
            <span>仅包含任务内 write/edit 与 Office 写入捕获的版本</span>
            <button
              type="button"
              className="fox-managed-refresh"
              onClick={() => void load()}
              disabled={loading}
              title="重新扫描"
            >
              {loading ? <LoaderCircle size={12} className="animate-spin" /> : <RefreshCw size={12} />}
            </button>
          </div>
          {groups.map((group) => (
            <div key={group.storagePath} className="fox-managed-group">
              <div className="fox-managed-file">
                <span className="fox-managed-name" title={group.storagePath}>
                  {group.displayName}
                </span>
                {group.drifted && (
                  <span className={`fox-managed-drift ${group.deleted ? 'is-deleted' : ''}`}>
                    <AlertTriangle size={11} />
                    {group.deleted ? '文件已被删除' : '之后被外部修改'}
                  </span>
                )}
              </div>
              <ul className="fox-managed-versions">
                {group.versions.map((version, index) => {
                  // One rule, shared with the tests: the row's own content size,
                  // and restore offered exactly when that content resolves.
                  const view = managedFileVersionPresentation(version, {
                    isLatest: index === 0,
                    drifted: group.drifted,
                  })
                  const isCurrent = view.current
                  const hint = forceHint[version.id]
                  return (
                    <li key={version.id} className="fox-managed-version">
                      <span className={`fox-managed-kind is-${version.changeKind}`}>
                        {KIND_COPY[version.changeKind] ?? version.changeKind}
                      </span>
                      <span className="fox-managed-no">v{version.versionNo}</span>
                      <span className="fox-managed-meta">
                        {version.tool} · {formatBytes(view.sizeLabel)} ·{' '}
                        {formatTime(version.createdAt)}
                      </span>
                      {version.changeKind === 'restored' && version.restoredFromId && (
                        <span className="fox-managed-source">来自先前版本</span>
                      )}
                      <span className="fox-managed-version-actions">
                        {isCurrent ? (
                          <span className="fox-managed-current">当前版本</span>
                        ) : (
                          <>
                            <Button
                              type="button"
                              size="sm"
                              variant="outline"
                              disabled={busyId === version.id || !view.restorable}
                              title={
                                view.restorable
                                  ? '把文件恢复为该版本记录的内容（当前内容会先备份）'
                                  : view.blocker ?? '该版本没有可验证的内容快照，无法恢复'
                              }
                              onClick={() => void restore(version, !!hint)}
                            >
                              {busyId === version.id ? (
                                <LoaderCircle size={12} className="animate-spin" />
                              ) : (
                                <RotateCcw size={12} />
                              )}
                              {hint ? '仍要覆盖恢复' : '恢复'}
                            </Button>
                          </>
                        )}
                      </span>
                      {!view.restorable && !isCurrent && view.blocker && (
                        <p className="fox-managed-blocker">{view.blocker}</p>
                      )}
                      {hint && (
                        <p className="fox-managed-force-hint">
                          {hint}
                          {!busyId && (
                            <button type="button" onClick={() => void restore(version, true)}>
                              强制恢复（先备份当前内容）
                            </button>
                          )}
                        </p>
                      )}
                    </li>
                  )
                })}
              </ul>
            </div>
          ))}
          {error && <p className="fox-managed-error">{error}</p>}
          <p className="fox-managed-footnote">
            注意：run_command 内任意命令、通用 MCP/远程服务造成的文件或外部副作用不在版本记录内，恢复不会重放任何工具调用。
          </p>
        </div>
      )}
    </div>
  )
}
