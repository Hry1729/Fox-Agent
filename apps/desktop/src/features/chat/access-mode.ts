// A1: the four execution modes of a Fox conversation.
//
// Stored as `permissionMode` (`read_only | ask | allow`) plus the conversation's
// `fullAccess` flag. The composer value keeps `allow` for auto so the project
// default (three stored modes) and the conversation value share one type.
import type { ConversationSummary, FoxAccessMode, ProjectRecord } from '@/features/conversations/model/types'

export type ComposerAccessMode = ProjectRecord['permissionMode'] | 'full_access'

/** [value, short label, menu label, description] in menu order. */
export const FOX_ACCESS_MODES: ReadonlyArray<readonly [ComposerAccessMode, string, string, string]> = [
  ['read_only', '只读', '只读模式', '只读取，不写文件、不执行命令'],
  ['ask', '询问', '执行前询问', '写入、命令和联网前都先问你'],
  ['allow', '自动', '自动执行', '项目内写入和低风险命令直接执行；高风险命令和受保护路径仍会询问'],
  ['full_access', '完全访问', '完全访问', '所有操作直接执行，包括项目外和受保护路径；内置禁止规则仍然生效'],
]

export function accessModeLabel(mode: ComposerAccessMode | null | undefined): string {
  return (FOX_ACCESS_MODES.find(([value]) => value === (mode ?? 'ask')) ?? FOX_ACCESS_MODES[1])[1]
}

/** The effective composer mode of a conversation. */
export function conversationAccessMode(conversation: Pick<ConversationSummary, 'permissionMode' | 'fullAccess'> | null | undefined): ComposerAccessMode | null {
  if (!conversation) return null
  return conversation.permissionMode === 'allow' && conversation.fullAccess === true ? 'full_access' : conversation.permissionMode
}

/** What the Host's four-mode command expects. */
export function hostAccessMode(mode: ComposerAccessMode): FoxAccessMode {
  return mode === 'allow' ? 'auto' : mode
}

/** The stored project default for a composer value (full access is never a project default). */
export function storedPermissionMode(mode: ComposerAccessMode): ProjectRecord['permissionMode'] {
  return mode === 'full_access' ? 'allow' : mode
}
