import { afterEach, beforeEach, describe, expect, mock, test } from 'bun:test'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { ContactSidebar } from '../src/features/companions/chat-controls'
import { ContactsContext, type ContactsContextValue } from '../src/features/companions/context'
import type { Companion } from '../src/features/companions/api'
import type { AgentRecord } from '../src/features/conversations/model/types'

type Call = { command: string; request: Record<string, unknown> }
const calls: Call[] = []
const opened: string[] = []
const refreshes = { count: 0 }
let createBlank: () => Promise<Companion>
let companionCounter = 0
const tauriCoreAsFound = { ...(await import('@tauri-apps/api/core')) }
const ok = (data: unknown) => ({ ok: true, data })
const companion = (id: string): Companion => ({
  id, conversationId: `chat-${id}`, templateAgentId: null, displayName: id,
  avatarRef: '', duty: '', personaOverride: null, workspaceDir: `/tmp/${id}`, workspaceState: 'ready', workspaceError: null,
  isDefault: false, pinnedAt: null, lastReadOrdinal: 0, modelProviderId: null, modelRef: null,
  reminderMode: 'follow_global', lastReadDeliveryOrder: null, manualUnreadFromOrder: null, manualUnreadAt: null,
  readDeliveryOrder: 0, pendingActionCount: 0, unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null,
})
const expert = (id: string, patch: Partial<AgentRecord> = {}) => ({ id, name: id, available: true, runtimeType: 'pi', agentKind: 'expert', ...patch }) as AgentRecord

mock.module('@tauri-apps/api/core', () => ({
  ...tauriCoreAsFound,
  invoke: async (command: string, args: unknown) => {
    const request = ((args as { request?: Record<string, unknown> })?.request ?? args) as Record<string, unknown>
    calls.push({ command, request })
    if (command === 'companion_create_blank') return ok(await createBlank())
    if (command === 'companion_add_from_expert') {
      companionCounter += 1
      return ok(companion(`created-${companionCounter}`))
    }
    if (command === 'companion_open') {
      const id = String(request.companionId)
      opened.push(id)
      return ok(companion(id))
    }
    if (command === 'agents_list') return ok([
      expert('compatible-a'), expert('compatible-b'),
      expert('unavailable', { available: false }),
      expert('remote', { runtimeType: 'yuxi' }),
      expert('not-expert', { agentKind: 'assistant' }),
    ])
    throw new Error(`Unexpected command: ${command}`)
  },
}))

function mount(collapsed = false) {
  const value = {
    current: undefined, privateChat: false, list: { companions: [], pendingDeletions: [] }, projects: [],
    model: '', permission: 'ask', error: '', refresh: async () => { refreshes.count += 1 }, refreshConversation: async () => {},
    open: () => {}, add: () => {}, showProfile: () => {}, leave: () => {}, setPermission: () => {}, openSharedMemory: () => {}, askToOrganize: () => {},
  } as ContactsContextValue
  return render(<ContactsContext.Provider value={value}><ContactSidebar collapsed={collapsed} /></ContactsContext.Provider>)
}

beforeEach(() => {
  calls.length = 0
  opened.length = 0
  refreshes.count = 0
  companionCounter = 0
  createBlank = async () => companion('blank')
})
afterEach(cleanup)

describe('唯一的小 Fox 创建入口', () => {
  test('右加号只有两项，折叠侧栏没有第二个创建入口', async () => {
    mount()
    fireEvent.pointerDown(screen.getByRole('button', { name: '新建小 Fox' }), { button: 0, ctrlKey: false, pointerType: 'mouse' })
    await waitFor(() => expect(screen.getAllByRole('menuitem')).toHaveLength(2))
    expect(screen.getAllByRole('menuitem').map(item => item.textContent)).toEqual(['新建小 Fox', '从专家创建'])
    cleanup()
    mount(true)
    expect(screen.queryByRole('button', { name: '新建小 Fox' })).toBeNull()
    expect(screen.queryByText('从专家创建')).toBeNull()
  })

  test('空白创建忙碌时去重，刷新后立即打开新固定会话', async () => {
    let resolve!: (value: Companion) => void
    createBlank = () => new Promise(value => { resolve = value })
    mount()
    fireEvent.pointerDown(screen.getByRole('button', { name: '新建小 Fox' }), { button: 0, ctrlKey: false, pointerType: 'mouse' })
    const item = await screen.findByRole('menuitem', { name: '新建小 Fox' })
    fireEvent.click(item)
    fireEvent.click(item)
    await waitFor(() => expect(calls.filter(call => call.command === 'companion_create_blank')).toHaveLength(1))
    expect(screen.getByRole('button', { name: '新建小 Fox' })).toHaveProperty('disabled', true)
    resolve(companion('blank'))
    await waitFor(() => expect(opened).toEqual(['blank']))
    expect(refreshes.count).toBe(1)
    expect(calls.filter(call => call.command === 'companion_create_blank')).toHaveLength(1)
  })

  test('专家只列兼容可用项，同一来源每次确认都新建并打开', async () => {
    mount()
    for (const expected of ['created-1', 'created-2']) {
      fireEvent.pointerDown(screen.getByRole('button', { name: '新建小 Fox' }), { button: 0, ctrlKey: false, pointerType: 'mouse' })
      fireEvent.click(await screen.findByRole('menuitem', { name: '从专家创建' }))
      await screen.findByRole('radio', { name: /compatible-a/ })
      expect(screen.queryByRole('radio', { name: /unavailable/ })).toBeNull()
      expect(screen.queryByRole('radio', { name: /remote/ })).toBeNull()
      expect(screen.queryByRole('radio', { name: /not-expert/ })).toBeNull()
      fireEvent.click(screen.getByRole('radio', { name: /compatible-a/ }))
      fireEvent.click(screen.getByRole('button', { name: '确认创建' }))
      await waitFor(() => expect(opened).toContain(expected))
    }
    const additions = calls.filter(call => call.command === 'companion_add_from_expert')
    expect(additions.map(call => call.request)).toEqual([{ expertId: 'compatible-a' }, { expertId: 'compatible-a' }])
    expect(opened).toEqual(['created-1', 'created-2'])
  })
})
