// sandbox.md 13.26 #3: a base URL already used by another service is caught
// before submit (normalized: trailing "/" and case ignored) with
// 「这个服务地址已经添加过了」 under 服务地址 and the submit disabled; whatever
// the Host returns, raw SQL text (UNIQUE constraint failed: …) is never shown.
import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const existing = {
  id: 'svc-a', name: '已有服务', icon: null, baseUrl: 'https://api.example.com/v1', apiType: 'openai-completions',
  isDefault: true, credentialConfigured: true, lastStatus: 'connected', lastLatencyMs: 1, lastCheckedAt: 1, lastError: null,
  protocolProfiles: [], createdAt: 1, updatedAt: 1,
  models: [{ id: 'm1', modelId: 'm', displayName: 'M', contextWindow: 8192, maxOutputTokens: 1024, supportsImageInput: false, isDefault: true }],
}
let saveError: unknown = null

const actualDesktopClient = { ...await import('../src/features/conversations/api/desktop-client') }
afterAll(() => { mock.module('../src/features/conversations/api/desktop-client', () => actualDesktopClient) })
mock.module('../src/features/conversations/api/desktop-client', () => ({
  ...actualDesktopClient,
  desktopRuntimeAvailable: true,
  desktopClient: {
    ...actualDesktopClient.desktopClient,
    listModelProviders: async () => [{ ...existing }],
    saveModelProvider: async () => { throw saveError },
    getModelServiceStatus: async () => null,
  },
}))

const React = await import('react')
const { act, cleanup, fireEvent, render, screen, waitFor, within } = await import('@testing-library/react')
const { ModelProvidersPage, ProviderEditor } = await import('../src/features/settings/settings-pages')
const { TooltipProvider } = await import('../src/components/ui/tooltip')
const { DUPLICATE_SERVICE_URL_TEXT, findDuplicateServiceUrl, providerErrorText, serviceUrlKey } = await import('../src/features/settings/provider-errors')
const { DesktopCommandError } = actualDesktopClient as any

afterEach(() => { cleanup(); saveError = null })

const form = (baseUrl: string, id?: string) => ({ id, name: '新服务', icon: null, baseUrl, apiType: 'openai-completions' as const, isDefault: false, apiKey: '', clearApiKey: false, protocolProfiles: [], models: [{ modelId: 'm', displayName: 'M', contextWindow: 8192, maxOutputTokens: 1024, supportsImageInput: false, isDefault: true }] })
const editor = (baseUrl: string, id?: string) => render(React.createElement(TooltipProvider, null, React.createElement(ProviderEditor, { layout: 'dialog', existingServices: [existing], form: form(baseUrl, id) as any, busy: false, credentialConfigured: false, onChange: () => {}, onCancel: () => {}, onSave: () => {}, onTest: () => {} })))

describe('duplicate service base URL', () => {
  test('normalized comparison ignores trailing slashes, case and the service itself', () => {
    expect(serviceUrlKey(' https://API.example.com/v1/ ')).toBe('https://api.example.com/v1')
    expect(findDuplicateServiceUrl([existing], undefined, 'https://api.example.com/v1/')?.id).toBe('svc-a')
    expect(findDuplicateServiceUrl([existing], 'svc-a', 'https://api.example.com/v1')).toBeNull()
    expect(findDuplicateServiceUrl([existing], undefined, 'https://api.example.com/v2')).toBeNull()
    expect(findDuplicateServiceUrl([existing], undefined, '')).toBeNull()
  })

  test('the editor shows the hint under 服务地址 and disables 添加', () => {
    editor('https://API.example.com/v1/')
    const hint = screen.getByText(DUPLICATE_SERVICE_URL_TEXT)
    expect(hint.id).toBe('fox-provider-base-url-error')
    expect((screen.getByRole('button', { name: '添加' }) as HTMLButtonElement).disabled).toBe(true)
  })

  test('a different URL, or the service being edited, has no hint', () => {
    editor('https://api.example.com/v2')
    expect(screen.queryByText(DUPLICATE_SERVICE_URL_TEXT)).toBeNull()
    expect((screen.getByRole('button', { name: '添加' }) as HTMLButtonElement).disabled).toBe(false)
    cleanup()
    editor('https://api.example.com/v1', 'svc-a')
    expect(screen.queryByText(DUPLICATE_SERVICE_URL_TEXT)).toBeNull()
  })

  test('error text never shows raw SQL', () => {
    expect(providerErrorText(new DesktopCommandError({ code: 'model.duplicate_base_url', message: '这个服务地址已经添加过了', retryable: false }))).toBe(DUPLICATE_SERVICE_URL_TEXT)
    expect(providerErrorText(new DesktopCommandError({ code: 'storage.operation_failed', message: 'UNIQUE constraint failed: model_providers.base_url', retryable: true }))).toBe(DUPLICATE_SERVICE_URL_TEXT)
    expect(providerErrorText(new Error('UNIQUE constraint failed: provider_models.provider_id, provider_models.model_id'))).toBe('保存失败，请稍后重试')
    expect(providerErrorText(new Error('attempt to write a readonly database'))).toBe('保存失败，请稍后重试')
    expect(providerErrorText(new Error('不支持此 API 协议'))).toBe('不支持此 API 协议')
  })

  test('a Host UNIQUE error on save is shown as the readable hint', async () => {
    saveError = new DesktopCommandError({ code: 'storage.operation_failed', message: 'UNIQUE constraint failed: model_providers.base_url', retryable: true })
    await act(async () => { render(React.createElement(TooltipProvider, null, React.createElement(ModelProvidersPage, { sidebarCollapsed: false, onSidebar: () => {} }))) })
    await waitFor(() => { if (!document.querySelector('[aria-label="编辑 已有服务"]')) throw new Error('not loaded') })
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: '编辑 已有服务' })) })
    const sheet = await waitFor(() => document.querySelector('[data-slot="sheet-content"]') as HTMLElement)
    await act(async () => { fireEvent.click(within(sheet).getByRole('button', { name: '保存' })) })
    await waitFor(() => expect(sheet.textContent).toContain(DUPLICATE_SERVICE_URL_TEXT))
    expect(document.body.textContent).not.toContain('UNIQUE constraint')
  })
})
