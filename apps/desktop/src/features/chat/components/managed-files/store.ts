// One refcounted source of managed-file versions per conversation. Both the
// contact card 资料库 and the right sidebar 文件 page subscribe to the same
// source, so opening both surfaces never doubles the IPC calls, and a restore
// started in one surface shows its busy state in the other. There is no
// interval polling: loads happen on first subscribe, on silent re-checks when
// a mount or a runtime signal says the data may be stale, on an explicit
// refresh, and after a restore.
import {
  desktopClient,
  desktopErrorDetails,
  desktopRuntimeAvailable,
  type ManagedFileVersion,
} from '@/features/conversations/api/desktop-client'
import { notify as toast } from '@/features/notifications'
import { confirmDialog } from '@/components/ui/confirm-dialog'
import { RestoreRequestIdentities, restoreActionKey } from '../restore-request-identity'

export interface ManagedFilesSnapshot {
  rows: ManagedFileVersion[]
  /** The very first read for this conversation is still in flight. */
  loading: boolean
  error: string | null
  /** The version whose restore is in flight, across every surface. */
  busyId: string | null
  /** Restore failures that force-answer (drift or deletion), by version id. */
  forceHint: Record<string, string>
}

type Listener = (snapshot: ManagedFilesSnapshot) => void

interface ManagedFileSource extends ManagedFilesSnapshot {
  conversationId: string
  listeners: Set<Listener>
  /** Bumped per accepted load; a late response for an older sequence is dropped. */
  seq: number
  inflight: boolean
  restoreIdentities: RestoreRequestIdentities
}

const sources = new Map<string, ManagedFileSource>()

function createSource(conversationId: string): ManagedFileSource {
  return {
    rows: [],
    loading: true,
    error: null,
    busyId: null,
    forceHint: {},
    conversationId,
    listeners: new Set(),
    seq: 0,
    inflight: false,
    restoreIdentities: new RestoreRequestIdentities(),
  }
}

function sourceOf(conversationId: string): ManagedFileSource {
  let source = sources.get(conversationId)
  if (!source) {
    source = createSource(conversationId)
    sources.set(conversationId, source)
  }
  return source
}

function publish(source: ManagedFileSource) {
  const snapshot: ManagedFilesSnapshot = {
    rows: source.rows,
    loading: source.loading,
    error: source.error,
    busyId: source.busyId,
    forceHint: source.forceHint,
  }
  for (const listener of [...source.listeners]) listener(snapshot)
}

async function load(source: ManagedFileSource, silent: boolean) {
  if (!desktopRuntimeAvailable) return
  if (source.inflight) return
  source.inflight = true
  const seq = ++source.seq
  if (!silent) {
    source.loading = source.rows.length === 0
    publish(source)
  }
  try {
    const rows = await desktopClient.listManagedFileVersions(source.conversationId)
    if (seq !== source.seq) return
    source.rows = rows
    source.error = null
    source.loading = false
  } catch (cause) {
    // A silent re-check never replaces shown data with an error; the user can
    // still restore what is on screen and retry manually.
    if (seq !== source.seq || silent) return
    source.error = desktopErrorDetails(cause).message
    source.loading = false
  } finally {
    if (seq === source.seq) source.inflight = false
    publish(source)
  }
}

export function subscribeManagedFiles(conversationId: string, listener: Listener): () => void {
  const source = sourceOf(conversationId)
  source.listeners.add(listener)
  listener({ rows: source.rows, loading: source.loading, error: source.error, busyId: source.busyId, forceHint: source.forceHint })
  void load(source, source.rows.length > 0)
  return () => { source.listeners.delete(listener) }
}

/** Manual 「重新检查」: shows the loading/error state instead of staying silent. */
export function refreshManagedFiles(conversationId: string) {
  const source = sources.get(conversationId)
  if (source) void load(source, false)
}

/** Mount/runtime-signal re-check: keeps shown data, updates when it changed. */
export function revalidateManagedFiles(conversationId: string) {
  const source = sources.get(conversationId)
  if (source) void load(source, true)
}

/**
 * Restore one version. `force` answers a drift/deletion rejection: the Host
 * backs the current bytes up first, then overwrites. The confirmation dialog
 * is the only await before the busy flag, so a double click is one request;
 * the request identity behind it is reused until the action ends undecided.
 */
export async function restoreManagedFile(
  conversationId: string,
  version: ManagedFileVersion,
  force: boolean,
  isCurrent: () => boolean = () => true,
): Promise<void> {
  const source = sourceOf(conversationId)
  if (source.busyId) return
  if (force) {
    const confirmed = await confirmDialog({
      title: '强制恢复文件',
      description: `文件 ${version.displayName} 在任务之外被修改过。\n强制恢复会先把当前内容备份，再覆盖为所选版本。是否继续？`,
      confirmLabel: '强制恢复',
      destructive: true,
    })
    // The surface may have switched conversation while the dialog was open.
    if (!confirmed || !isCurrent()) return
  }
  source.busyId = version.id
  publish(source)
  try {
    // The action is "restore THIS version, with or without force". Whether the
    // same version was restored before is irrelevant: each confirmation is a
    // new action once the previous one has ended.
    const actionKey = restoreActionKey(version.id, force)
    const requestId = source.restoreIdentities.acquire(
      actionKey,
      () => globalThis.crypto?.randomUUID?.() ?? `restore-${Date.now()}-${Math.random().toString(16).slice(2)}`,
    )
    const restored = await desktopClient.restoreManagedFileVersion(conversationId, version.id, force, requestId)
    // Definite end: release the identity so the next confirmation is a new
    // action rather than a resend of this one.
    source.restoreIdentities.release(actionKey)
    const nextHint = { ...source.forceHint }
    delete nextHint[version.id]
    source.forceHint = nextHint
    await load(source, true)
    toast.success(`已恢复 ${version.displayName} 到 v${restored.versionNo}，当前内容已自动备份`)
  } catch (cause) {
    const details = desktopErrorDetails(cause)
    source.error = details.message
    // Drift/deletion rejections are exactly the cases force handles.
    if (!force) source.forceHint = { ...source.forceHint, [version.id]: details.message }
  } finally {
    source.busyId = null
    publish(source)
  }
}
