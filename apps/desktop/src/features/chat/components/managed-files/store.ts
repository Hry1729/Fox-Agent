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
  restoreError: { message: string; version: ManagedFileVersion; force: boolean } | null
  /** The version whose restore is in flight, across every surface. */
  busyId: string | null
  /** Restore failures that force-answer (drift or deletion), by version id. */
  forceHint: Record<string, string>
}

type Listener = (snapshot: ManagedFilesSnapshot) => void

interface ManagedFileSource extends ManagedFilesSnapshot {
  conversationId: string
  listeners: Set<Listener>
  reading: Promise<void> | null
  pendingSilent: boolean | null
  confirming: boolean
  restoreIdentities: RestoreRequestIdentities
}

const sources = new Map<string, ManagedFileSource>()

function createSource(conversationId: string): ManagedFileSource {
  return {
    rows: [],
    loading: true,
    error: null,
    restoreError: null,
    busyId: null,
    forceHint: {},
    conversationId,
    listeners: new Set(),
    reading: null,
    pendingSilent: null,
    confirming: false,
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

function snapshotOf(source: ManagedFileSource): ManagedFilesSnapshot {
  return {
    rows: source.rows,
    loading: source.loading,
    error: source.error,
    restoreError: source.restoreError,
    busyId: source.busyId,
    forceHint: source.forceHint,
  }
}

function publish(source: ManagedFileSource) {
  const snapshot = snapshotOf(source)
  for (const listener of [...source.listeners]) listener(snapshot)
}

function load(source: ManagedFileSource, silent: boolean): Promise<void> {
  source.pendingSilent = (source.pendingSilent ?? true) && silent
  if (!source.reading) {
    source.reading = Promise.resolve().then(async () => {
      try {
        while (source.pendingSilent !== null) {
          const silentRead = source.pendingSilent
          source.pendingSilent = null
          if (!silentRead && source.rows.length === 0) {
            source.loading = true
            publish(source)
          }
          try {
            source.rows = await desktopClient.listManagedFileVersions(source.conversationId)
            source.error = null
          } catch (cause) {
            if (!silentRead) source.error = desktopErrorDetails(cause).message
          }
          source.loading = false
          publish(source)
        }
      } finally {
        source.reading = null
      }
    })
  }
  return source.reading
}

export function subscribeManagedFiles(conversationId: string, listener: Listener): () => void {
  const source = sourceOf(conversationId)
  const firstSubscriber = source.listeners.size === 0
  source.listeners.add(listener)
  listener(snapshotOf(source))
  if (desktopRuntimeAvailable && firstSubscriber && !source.reading) {
    void load(source, source.rows.length > 0)
  }
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
 * backs the current bytes up first, then overwrites.
 */
export async function restoreManagedFile(
  conversationId: string,
  version: ManagedFileVersion,
  force: boolean,
  isCurrent: () => boolean = () => true,
): Promise<void> {
  const source = sourceOf(conversationId)
  if (source.busyId || !isCurrent()) return
  if (force) {
    if (source.confirming) return
    source.confirming = true
    const confirmed = await confirmDialog({
      title: '强制恢复文件',
      description: `文件 ${version.displayName} 在任务之外被修改过。\n强制恢复会先把当前内容备份，再覆盖为所选版本。是否继续？`,
      confirmLabel: '强制恢复',
      destructive: true,
    })
    source.confirming = false
    if (!confirmed || !isCurrent() || source.busyId) return
  }
  source.busyId = version.id
  source.restoreError = null
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
    await load(source, false)
    toast.success(`已恢复 ${version.displayName} 到 v${restored.versionNo}，当前内容已自动备份`)
  } catch (cause) {
    const details = desktopErrorDetails(cause)
    const requiresForce = !force && (
      details.message.includes('changed outside the recorded task history') ||
      details.message.includes('was deleted after the last recorded change')
    )
    source.restoreError = { message: details.message, version, force: force || requiresForce }
    if (requiresForce) source.forceHint = { ...source.forceHint, [version.id]: details.message }
  } finally {
    source.busyId = null
    publish(source)
  }
}
