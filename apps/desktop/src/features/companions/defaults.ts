import { hostAccessMode } from '@/features/chat/access-mode'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { ProjectRecord } from '@/features/conversations/model/types'
import { contacts, type Companion, type ReminderMode } from './api'

export const COMPANION_DEFAULT_PERMISSION_KEY = 'fox.preferences.companionDefaultPermission'
export const COMPANION_DEFAULT_MODEL_KEY = 'fox.preferences.companionDefaultModel'
export const COMPANION_DEFAULT_REMINDER_KEY = 'fox.preferences.companionDefaultReminder'

export type CompanionPermission = ProjectRecord['permissionMode']

function stored(key: string) {
  return typeof window === 'undefined' ? null : window.localStorage.getItem(key)
}

export function readCompanionDefaultPermission(): CompanionPermission {
  const value = stored(COMPANION_DEFAULT_PERMISSION_KEY)
  return value === 'read_only' || value === 'allow' || value === 'ask' ? value : 'ask'
}

export function readCompanionDefaultModel() {
  return stored(COMPANION_DEFAULT_MODEL_KEY) ?? ''
}

export function readCompanionDefaultReminder(): ReminderMode {
  const value = stored(COMPANION_DEFAULT_REMINDER_KEY)
  return value === 'all' || value === 'action_only' || value === 'muted' || value === 'follow_global' ? value : 'follow_global'
}

/**
 * Applies the preferences to a newly created companion through the same IPC
 * calls as its profile page. Existing companions are never touched. A partial
 * failure is returned to the caller: creation has already succeeded and must
 * not be presented as though it rolled back.
 */
export async function applyCompanionDefaults(contact: Companion) {
  let current = contact
  const errors: string[] = []
  const model = readCompanionDefaultModel()
  if (model) {
    const separator = model.indexOf('::')
    if (separator > 0) {
      try {
        current = await contacts.setModel(current.id, model.slice(0, separator), model.slice(separator + 2))
      } catch (cause) {
        errors.push(`默认模型：${cause instanceof Error ? cause.message : String(cause)}`)
      }
    }
  }
  const reminder = readCompanionDefaultReminder()
  if (reminder !== 'follow_global') {
    try {
      current = await contacts.setReminderMode(current.id, reminder)
    } catch (cause) {
      errors.push(`默认提醒：${cause instanceof Error ? cause.message : String(cause)}`)
    }
  }
  const permission = readCompanionDefaultPermission()
  if (permission !== 'ask') {
    try {
      const policy = await desktopClient.conversationPermissionState(current.conversationId)
      await desktopClient.updateConversationPermission(
        current.conversationId,
        hostAccessMode(permission),
        crypto.randomUUID(),
        policy.version,
      )
    } catch (cause) {
      errors.push(`默认执行方式：${cause instanceof Error ? cause.message : String(cause)}`)
    }
  }
  return { contact: current, errors }
}
