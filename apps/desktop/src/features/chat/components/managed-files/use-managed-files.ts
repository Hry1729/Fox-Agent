// Controller over the shared managed-files store for one surface. It owns the
// per-surface expansion state and turns the store snapshot into a discriminated
// state (loading / ready / error) that both the 资料库 rows and the right
// sidebar version list render from.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { desktopRuntimeAvailable, type ManagedFileVersion } from '@/features/conversations/api/desktop-client'
import {
  groupManagedFileVersions,
  managedFilesFingerprint,
  type ManagedFileGroup,
} from './model'
import {
  refreshManagedFiles,
  restoreManagedFile,
  revalidateManagedFiles,
  subscribeManagedFiles,
  type ManagedFilesSnapshot,
} from './store'

export type ManagedFilesState =
  | { status: 'loading' }
  /** Refresh failures keep the shown rows; `error` reports them inline. */
  | { status: 'ready'; groups: ManagedFileGroup[]; fingerprint: string; error: string | null }
  /** The first read failed and nothing can be shown. */
  | { status: 'error'; message: string }

export interface ManagedFilesController {
  conversationId: string
  state: ManagedFilesState
  busyId: string | null
  forceHint: Record<string, string>
  /** Expanded file groups, keyed by storage path (per surface instance). */
  expanded: Record<string, boolean>
  toggleExpanded(storagePath: string): void
  restore(version: ManagedFileVersion, force: boolean): void
  refresh(): void
}

export function useManagedFiles(
  conversationId: string | null | undefined,
  refreshKey?: unknown,
): ManagedFilesController | null {
  const activeId = desktopRuntimeAvailable && conversationId ? conversationId : null
  const [snapshot, setSnapshot] = useState<ManagedFilesSnapshot | null>(null)
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})
  const conversationIdRef = useRef(activeId)
  conversationIdRef.current = activeId

  useEffect(() => {
    if (!activeId) {
      setSnapshot(null)
      return
    }
    setExpanded({})
    return subscribeManagedFiles(activeId, setSnapshot)
  }, [activeId])

  // Event-driven re-check: a mount or a runtime signal (run state, message or
  // artifact count) says the registry may have grown. Silent, so shown data is
  // never replaced by a skeleton; the store deduplicates overlapping loads.
  useEffect(() => {
    if (!activeId || refreshKey === undefined) return
    revalidateManagedFiles(activeId)
  }, [activeId, refreshKey])

  const state = useMemo<ManagedFilesState>(() => {
    if (!snapshot) return { status: 'loading' }
    if (snapshot.error && snapshot.rows.length === 0) return { status: 'error', message: snapshot.error }
    const groups = groupManagedFileVersions(snapshot.rows)
    return {
      status: 'ready',
      groups,
      fingerprint: managedFilesFingerprint(groups),
      error: snapshot.error,
    }
  }, [snapshot])

  const toggleExpanded = useCallback((storagePath: string) => {
    setExpanded(current => ({ ...current, [storagePath]: !current[storagePath] }))
  }, [])

  const restore = useCallback((version: ManagedFileVersion, force: boolean) => {
    if (!activeId) return
    const id = activeId
    void restoreManagedFile(id, version, force, () => conversationIdRef.current === id)
  }, [activeId])

  const refresh = useCallback(() => {
    if (activeId) refreshManagedFiles(activeId)
  }, [activeId])

  if (!activeId || !snapshot) return null
  return {
    conversationId: activeId,
    state,
    busyId: snapshot.busyId,
    forceHint: snapshot.forceHint,
    expanded,
    toggleExpanded,
    restore,
    refresh,
  }
}

/** The fingerprint whose unseen state is worth checking, or null while loading. */
export function managedFilesFingerprintOf(controller: ManagedFilesController | null | undefined): string | null {
  if (!controller || controller.state.status === 'loading') return null
  return controller.state.status === 'ready' ? controller.state.fingerprint : null
}
