import { useState } from 'react'
import type { WorkspaceView } from '@/features/workspace/types'

/** Small helpers shared by the settings pages (通用 … 数据与关于). */

export function serviceStatusLabel(status?: string) {
  if (status === 'connected') return '已连接'
  if (status === 'unavailable') return '不可用'
  return '未检测'
}

export function knowledgeServiceName(name?: string | null) {
  const value = name?.trim()
  if (!value || /^yuxi(?:\s*(?:service|服务))?$/i.test(value)) return '资料库服务'
  return value
}

export function connectionTypeLabel(type?: string) {
  if (type === 'local') return '本机服务'
  if (type === 'lan') return '局域网服务'
  return '远程服务'
}

export function formatStorageSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(bytes < 10 * 1024 * 1024 ? 1 : 0)} MB`
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`
}

export function useStoredPreference<T extends string | boolean>(key: string, fallback: T) {
  const [value, setValue] = useState<T>(() => {
    if (typeof window === 'undefined') return fallback
    const stored = window.localStorage.getItem(key)
    if (stored === null) return fallback
    if (typeof fallback === 'boolean') return (stored === '1') as T
    return stored as T
  })
  const update = (next: T) => {
    setValue(next)
    window.localStorage.setItem(key, typeof next === 'boolean' ? next ? '1' : '0' : next)
  }
  return [value, update] as const
}

/**
 * 「添加技能 / 添加连接器」 opened from an expert: the 插件 page shows a way back
 * to that expert instead of dropping the user on a bare plugin list. Stored in
 * sessionStorage right before navigating (same pattern as the 资料库 service
 * return key) and cleared once the 插件 page has read it.
 */
export const PLUGIN_RETURN_KEY = 'fox.settings.pluginReturn'

export interface PluginReturnTarget {
  view: WorkspaceView
  entityId?: string
  documentId?: string
  label: string
}

export function rememberPluginReturn(target: PluginReturnTarget) {
  try { window.sessionStorage.setItem(PLUGIN_RETURN_KEY, JSON.stringify(target)) } catch { /* storage unavailable: no way back, plugin page still opens */ }
}

/** Reads the stored way back without clearing it (safe in a state initializer). */
export function readPluginReturn(): PluginReturnTarget | null {
  try {
    const raw = window.sessionStorage.getItem(PLUGIN_RETURN_KEY)
    if (!raw) return null
    const value = JSON.parse(raw) as Partial<PluginReturnTarget>
    if (typeof value.view !== 'string' || typeof value.label !== 'string') return null
    return { view: value.view as WorkspaceView, entityId: typeof value.entityId === 'string' ? value.entityId : undefined, documentId: typeof value.documentId === 'string' ? value.documentId : undefined, label: value.label }
  } catch {
    return null
  }
}

export function clearPluginReturn() {
  try { window.sessionStorage.removeItem(PLUGIN_RETURN_KEY) } catch { /* nothing stored */ }
}
