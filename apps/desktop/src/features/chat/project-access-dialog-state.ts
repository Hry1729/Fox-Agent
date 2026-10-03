import type { ProjectRecord } from '@/features/conversations/model/types'

export interface ProjectAccessSelection {
  path: string
  permissionMode: ProjectRecord['permissionMode']
}

export function normalizeProjectPermission(value: unknown, fallback: ProjectRecord['permissionMode'] = 'ask'): ProjectRecord['permissionMode'] {
  return value === 'read_only' || value === 'ask' || value === 'allow' ? value : fallback
}

/**
 * Identity of a project is its full path, never its folder name: two projects can
 * share a leaf directory, and the same project can arrive with different separators
 * or a trailing separator. Kept in one place so grouping, permission lookup and
 * draft creation cannot disagree about which project they mean.
 */
export function normalizeProjectRoot(path: string) {
  return canonicalProjectRoot(path).replace(/\//g, '\\').toLocaleLowerCase()
}

/**
 * The single written form of a project path, so one directory can never be recorded
 * twice. A trailing separator is dropped because Windows treats it as decoration —
 * except on a drive root, where `D:\` is the root and `D:` is "the current
 * directory on D", so that separator has to survive.
 */
export function canonicalProjectRoot(path: string) {
  const trimmed = path.trim()
  if (/^[a-zA-Z]:[\\/]*$/.test(trimmed)) return `${trimmed.slice(0, 2)}\\`
  return trimmed.replace(/[\\/]+$/, '')
}

export function matchingProject(path: string, projects: ProjectRecord[]) {
  const normalized = normalizeProjectRoot(path)
  if (!normalized) return undefined
  return projects.find((project) => normalizeProjectRoot(project.rootPath) === normalized)
}

export function selectProjectRoot(
  path: string,
  projects: ProjectRecord[],
  fallbackPermission: ProjectRecord['permissionMode'],
): ProjectAccessSelection {
  return {
    path: canonicalProjectRoot(path),
    permissionMode: matchingProject(path, projects)?.permissionMode ?? fallbackPermission,
  }
}

export function validPickedProjectFolder(value: unknown) {
  if (value === null) return null
  if (typeof value !== 'string' || !value.trim()) {
    throw new Error('系统文件夹选择器没有返回有效路径，请重试')
  }
  return value.trim()
}
