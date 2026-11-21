import { afterAll, beforeEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()

const actualApi = { ...await import('../src/features/companions/api') }
const actualDesktop = { ...await import('../src/features/conversations/api/desktop-client') }
const calls: unknown[][] = []
let failModel = false

mock.module('../src/features/companions/api', () => ({
  ...actualApi,
  contacts: {
    ...actualApi.contacts,
    setModel: async (id: string, providerId: string, modelRef: string) => {
      calls.push(['model', id, providerId, modelRef])
      if (failModel) throw new Error('模型不可用')
      return { ...contact, modelProviderId: providerId, modelRef }
    },
    setReminderMode: async (id: string, mode: string) => {
      calls.push(['reminder', id, mode])
      return { ...contact, reminderMode: mode }
    },
  },
}))
mock.module('../src/features/conversations/api/desktop-client', () => ({
  ...actualDesktop,
  desktopClient: {
    ...actualDesktop.desktopClient,
    conversationPermissionState: async (conversationId: string) => {
      calls.push(['permissionState', conversationId])
      return { version: 7 }
    },
    updateConversationPermission: async (conversationId: string, permission: string, requestId: string, expectedVersion: number) => {
      calls.push(['permission', conversationId, permission, requestId, expectedVersion])
      return {}
    },
  },
}))
afterAll(() => {
  mock.module('../src/features/companions/api', () => actualApi)
  mock.module('../src/features/conversations/api/desktop-client', () => actualDesktop)
})

const { applyCompanionDefaults, readCompanionDefaultPermission, readCompanionDefaultReminder } = await import('../src/features/companions/defaults')
const contact = { id: 'cmp_test', conversationId: 'conv_test', workspaceDir: 'D:\\Fox\\companions\\cmp_test\\workspace' } as import('../src/features/companions/api').Companion

beforeEach(() => {
  calls.length = 0
  failModel = false
  window.localStorage.clear()
})

describe('new companion defaults', () => {
  test('rejects unknown stored values and leaves default creation unchanged', async () => {
    window.localStorage.setItem('fox.preferences.companionDefaultPermission', 'everything')
    window.localStorage.setItem('fox.preferences.companionDefaultReminder', 'loud')
    expect(readCompanionDefaultPermission()).toBe('ask')
    expect(readCompanionDefaultReminder()).toBe('follow_global')
    const result = await applyCompanionDefaults(contact)
    expect(calls).toEqual([])
    expect(result.errors).toEqual([])
  })

  test('uses the real model, reminder and conversation permission APIs', async () => {
    window.localStorage.setItem('fox.preferences.companionDefaultModel', 'provider-1::model-1')
    window.localStorage.setItem('fox.preferences.companionDefaultPermission', 'read_only')
    window.localStorage.setItem('fox.preferences.companionDefaultReminder', 'action_only')
    const result = await applyCompanionDefaults(contact)
    expect(calls).toEqual([
      ['model', 'cmp_test', 'provider-1', 'model-1'],
      ['reminder', 'cmp_test', 'action_only'],
      ['permissionState', 'conv_test'],
      ['permission', 'conv_test', 'read_only', expect.any(String), 7],
    ])
    expect(result.errors).toEqual([])
  })

  test('maps automatic mode to the Host access-mode contract', async () => {
    window.localStorage.setItem('fox.preferences.companionDefaultPermission', 'allow')
    const result = await applyCompanionDefaults(contact)
    expect(calls).toContainEqual(['permission', 'conv_test', 'auto', expect.any(String), 7])
    expect(result.errors).toEqual([])
  })

  test('reports partial failure without pretending creation rolled back', async () => {
    failModel = true
    window.localStorage.setItem('fox.preferences.companionDefaultModel', 'provider-1::model-1')
    window.localStorage.setItem('fox.preferences.companionDefaultReminder', 'muted')
    const result = await applyCompanionDefaults(contact)
    expect(result.errors).toEqual(['默认模型：模型不可用'])
    expect(calls).toContainEqual(['reminder', 'cmp_test', 'muted'])
  })
})
