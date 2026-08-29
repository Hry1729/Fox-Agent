import type { ProjectRecord } from '@/features/conversations/model/types'

export interface ProjectAccessSelection {
  path: string
  permissionMode: ProjectRecord['permissionMode']
}

export function normalizeProjectPermission(value: unknown, fallback: ProjectRecord['permissionMode'] = 'ask'): ProjectRecord['permissionMode'] {
  return value === 'read_only' || value === 'ask' || value === 'allow' ? value : fallback
}

function normalizedProjectRoot(path: string) {
  return path.trim().replace(/[\\/]+$/, '').toLocaleLowerCase()
}

function matchingProject(path: string, projects: ProjectRecord[]) {
  const normalized = normalizedProjectRoot(path)
  if (!normalized) return undefined
  return projects.find((project) => normalizedProjectRoot(project.rootPath) === normalized)
}

export function selectProjectRoot(
  path: string,
  projects: ProjectRecord[],
  fallbackPermission: ProjectRecord['permissionMode'],
): ProjectAccessSelection {
  return {
    path,
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
