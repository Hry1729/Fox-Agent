import { afterEach, describe, expect, test } from 'bun:test'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks'
import { ContactsContext, type ContactsContextValue } from '../src/features/companions/context'
import { ContactModelSelect, overrideUnavailableReason } from '../src/features/companions/model-select'
import type { Companion } from '../src/features/companions/api'
import type { ModelProviderRecord } from '../src/features/conversations/model/types'

const contact: Companion = {
  id: 'contact', conversationId: 'chat', templateAgentId: 'expert', displayName: '资料伙伴', avatarRef: '📚',
  duty: '', personaOverride: null, workspaceDir: 'D:/contact', workspaceState: 'ready', workspaceError: null,
  isDefault: false, pinnedAt: null, lastReadOrdinal: 0, modelProviderId: null, modelRef: null,
  unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null,
}
const provider = (patch: Partial<ModelProviderRecord> = {}): ModelProviderRecord => ({
  id: 'prov', name: '独立服务', enabled: true,
  models: [{ id: 'prov-model', modelId: 'independent-model', displayName: '独立模型', contextWindow: 64000, maxOutputTokens: 4096, supportsImageInput: true, isDefault: true }],
  ...patch,
} as ModelProviderRecord)
const context = (current: Companion, refresh = async () => {}): ContactsContextValue => ({
  current, privateChat: true, list: { companions: [current], pendingDeletions: [] }, projects: [], model: '全局模型', permission: 'ask', error: '',
  refresh, refreshConversation: async () => {}, open: () => {}, add: () => {}, showProfile: () => {},
  leave: () => {}, setPermission: () => {}, openSharedMemory: () => {}, askToOrganize: () => {},
})

afterEach(() => { cleanup(); clearMocks() })

describe('联系人独立模型', () => {
  test('不可用原因：服务删除、停用、模型移除分别说明，可用或跟随全局时无原因', () => {
    const providers = [provider()]
    expect(overrideUnavailableReason(contact, providers)).toBeNull()
    expect(overrideUnavailableReason({ ...contact, modelProviderId: 'prov', modelRef: 'prov-model' }, providers)).toBeNull()
    expect(overrideUnavailableReason({ ...contact, modelProviderId: 'gone', modelRef: 'm' }, providers)).toContain('删除')
    expect(overrideUnavailableReason({ ...contact, modelProviderId: 'prov', modelRef: 'prov-model' }, [provider({ enabled: false })])).toContain('停用')
    expect(overrideUnavailableReason({ ...contact, modelProviderId: 'prov', modelRef: 'gone' }, providers)).toContain('移除')
  })

  test('默认跟随全局，选择独立模型写入服务与模型引用，改回跟随全局清空', async () => {
    const calls: Record<string, unknown>[] = []
    let refreshed = 0
    mockIPC((command, args) => {
      if (command === 'model_providers_list') return { ok: true, data: [provider(), provider({ id: 'off', name: '停用服务', enabled: false })] }
      if (command === 'companion_set_model') {
        calls.push(args as Record<string, unknown>)
        return { ok: true, data: contact }
      }
      throw new Error('Unexpected command: ' + command)
    })
    const view = render(<ContactsContext.Provider value={context(contact, async () => { refreshed += 1 })}>
      <ContactModelSelect contact={contact} />
    </ContactsContext.Provider>)
    const select = await waitFor(() => view.getByLabelText('联系人模型') as HTMLSelectElement)
    await waitFor(() => expect(select.disabled).toBe(false))
    expect(select.value).toBe('')
    expect(view.getByText('现在是 全局模型，全局换了它也跟着换')).toBeDefined()
    const options = [...select.querySelectorAll('option')].map(option => option.textContent)
    expect(options[0]).toContain('跟随全局')
    expect(options[0]).toContain('全局模型')
    expect(options).toContain('独立模型 · 能看图')
    expect(select.querySelector('optgroup')?.label).toBe('独立服务')
    const disabledGroups = [...select.querySelectorAll('optgroup')].filter(group => group.disabled)
    expect(disabledGroups).toHaveLength(1)
    expect(disabledGroups[0].label).toContain('停用服务')

    fireEvent.change(select, { target: { value: 'prov:prov-model' } })
    await waitFor(() => expect(calls).toEqual([{ request: { companionId: 'contact', providerId: 'prov', modelRef: 'prov-model' } }]))
    expect(refreshed).toBe(1)
    fireEvent.change(select, { target: { value: '' } })
    await waitFor(() => expect(calls).toHaveLength(2))
    expect(calls[1]).toEqual({ request: { companionId: 'contact', providerId: null, modelRef: null } })
  })

  test('已保存的覆盖失效时显示原因而不是静默换模型', async () => {
    mockIPC(command => {
      if (command === 'model_providers_list') return { ok: true, data: [provider({ enabled: false })] }
      throw new Error('Unexpected command: ' + command)
    })
    const broken = { ...contact, modelProviderId: 'prov', modelRef: 'prov-model' }
    const view = render(<ContactsContext.Provider value={context(broken)}>
      <ContactModelSelect contact={broken} />
    </ContactsContext.Provider>)
    await waitFor(() => expect(view.getByRole('alert').textContent).toContain('停用'))
    const select = view.getByLabelText('联系人模型') as HTMLSelectElement
    expect(select.value).toBe('__unavailable__')
  })
})