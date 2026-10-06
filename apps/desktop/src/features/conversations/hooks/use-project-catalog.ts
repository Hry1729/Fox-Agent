import { useCallback, useEffect, useRef, useState } from 'react'
import { desktopClient } from '../api/desktop-client'
import type { ProjectRecord } from '../model/types'

/** Pin writes invalidate older list reads, so a late refresh cannot undo the menu. */
export function useProjectCatalog(enabled: boolean, activeProjectId?: string | null) {
  const [projects, setProjects] = useState<ProjectRecord[]>([])
  const requestVersion = useRef(0)
  const refreshProjects = useCallback(async () => {
    const version = ++requestVersion.current
    const records = await desktopClient.listProjects()
    if (version === requestVersion.current) setProjects(records)
  }, [])
  const setProjectPinned = useCallback(async (id: string, pinned: boolean) => {
    requestVersion.current += 1
    const updated = await desktopClient.setProjectPinned(id, pinned)
    requestVersion.current += 1
    setProjects(current => current.some(project => project.id === updated.id)
      ? current.map(project => project.id === updated.id
        ? { ...project, pinned: updated.pinned, updatedAt: Math.max(project.updatedAt, updated.updatedAt) }
        : project)
      : [...current, updated])
    return updated
  }, [])
  useEffect(() => {
    if (enabled) void refreshProjects().catch(() => undefined)
    return () => { requestVersion.current += 1 }
  }, [enabled, activeProjectId, refreshProjects])
  return { projects, setProjects, refreshProjects, setProjectPinned }
}
