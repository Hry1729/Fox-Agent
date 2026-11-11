import { afterEach, describe, expect, test } from 'bun:test'
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/react'
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
    const renderModel = (current: Companion) => <ContactsContext.Provider value={context(current, async () => { refreshed += 1 })}>
      <ContactModelSelect contact={current} />
    </ContactsContext.Provider>
    const view = render(renderModel(contact))
    const select = view.getByRole('combobox', { name: '联系人模型' }) as HTMLButtonElement
    await waitFor(() => expect(select.disabled).toBe(false))
    expect(within(select).getByText('全局模型')).toBeDefined()
    expect(within(select).getByText('跟随全局').tagName).toBe('SMALL')
    expect(view.queryByText(/全局换了它也跟着换/)).toBeNull()
    fireEvent.keyDown(select, { key: 'Enter' })
    const list = within(await view.findByRole('listbox'))
    const enabled = list.getByRole('group', { name: '独立服务' })
    const disabled = list.getByRole('group', { name: '停用服务（已停用）' })
    expect(within(disabled).getByRole('option').getAttribute('aria-disabled')).toBe('true')
    const modelOption = within(enabled).getByRole('option', { name: /独立模型.*能看图/ })
    expect(modelOption.querySelector('svg[aria-hidden="true"]')).not.toBeNull()
    fireEvent.click(modelOption)
    await waitFor(() => expect(calls).toEqual([{ request: { companionId: 'contact', providerId: 'prov', modelRef: 'prov-model' } }]))
    expect(refreshed).toBe(1)
    view.rerender(renderModel({ ...contact, modelProviderId: 'prov', modelRef: 'prov-model' }))
    expect(select.textContent).toBe('独立模型')
    expect(view.queryByText(/只影响下一轮对话/)).toBeNull()
    fireEvent.keyDown(select, { key: 'Enter' })
    fireEvent.click(within(await view.findByRole('listbox')).getByRole('option', { name: /全局模型.*跟随全局/ }))
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
    expect(view.getByRole('combobox', { name: '联系人模型' }).textContent).toBe('独立模型不可用')
  })
})
