// 模型 settings: 「添加服务」 opens a centred dialog (not the right sheet) with the
// template first, then 基础连接, then the collapsed 协议与高级, and 取消 / 添加 in
// the footer. Editing an existing service keeps the sheet layout (保存).
import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { cleanup, fireEvent, render, screen, waitFor, within } = await import('@testing-library/react')
const { ModelProvidersPage, ProviderEditor } = await import('../src/features/settings/settings-pages')
const { TooltipProvider } = await import('../src/components/ui/tooltip')

afterEach(() => cleanup())

function mountPage() {
  return render(React.createElement(TooltipProvider, null, React.createElement(ModelProvidersPage, { sidebarCollapsed: false, onSidebar: () => {} })))
}

describe('模型 settings: 添加服务 dialog', () => {
  test('opens a centred dialog in template → 基础连接 → 协议与高级 order, not the sheet', async () => {
    mountPage()
    fireEvent.click(screen.getByRole('button', { name: '添加服务' }))
    const dialog = await waitFor(() => screen.getByRole('dialog', { name: '添加服务' }))
    expect(dialog.getAttribute('data-slot')).toBe('dialog-content')
    expect(dialog.className).toContain('fox-provider-add-dialog')
    expect(document.querySelector('[data-slot="sheet-content"]')).toBeNull()

    const order = [...dialog.querySelectorAll('section[aria-label]')].map((section) => section.getAttribute('aria-label'))
    expect(order.indexOf('服务模板')).toBe(0)
    expect(order.indexOf('基础连接')).toBeGreaterThan(order.indexOf('服务模板'))
    expect(order.indexOf('协议与高级')).toBeGreaterThan(order.indexOf('基础连接'))

    const view = within(dialog)
    expect(view.getByRole('combobox', { name: '服务模板' })).toBeTruthy()
    expect(view.getByText('服务地址')).toBeTruthy()
    expect(view.getByLabelText('API Key')).toBeTruthy()
    expect(view.getByRole('button', { name: /测试连接/ })).toBeTruthy()
    const disclosure = view.getByRole('button', { name: /协议与高级/ })
    expect(disclosure.getAttribute('aria-expanded')).toBe('false')
    fireEvent.click(disclosure)
    expect(disclosure.getAttribute('aria-expanded')).toBe('true')
    expect(view.getByRole('combobox', { name: 'Fox 连接协议' })).toBeTruthy()

    expect(view.getByRole('button', { name: '取消' })).toBeTruthy()
    expect(view.getByRole('button', { name: '添加' })).toBeTruthy()
    expect(view.queryByRole('button', { name: '保存' })).toBeNull()
    expect(view.queryByRole('button', { name: '删除服务' })).toBeNull()
  })

  test('取消 closes the dialog', async () => {
    mountPage()
    fireEvent.click(screen.getByRole('button', { name: '添加服务' }))
    const dialog = await waitFor(() => screen.getByRole('dialog', { name: '添加服务' }))
    fireEvent.click(within(dialog).getByRole('button', { name: '取消' }))
    await waitFor(() => expect(screen.queryByRole('dialog', { name: '添加服务' })).toBeNull())
  })

  test('the editor keeps the sheet layout for an existing service', () => {
    const form = { id: 'p1', name: 'DeepSeek', icon: null, baseUrl: 'https://api.deepseek.com', apiType: 'openai-completions' as const, isDefault: true, apiKey: '', clearApiKey: false, protocolProfiles: [], models: [{ id: 'm1', modelId: 'deepseek-chat', displayName: 'DeepSeek Chat', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: false, isDefault: true }] }
    const { container } = render(React.createElement(TooltipProvider, null, React.createElement(ProviderEditor, { form: form as any, busy: false, credentialConfigured: true, onChange: () => {}, onCancel: () => {}, onSave: () => {}, onDelete: () => {}, onTest: () => {} })))
    expect(container.querySelector('.fox-settings-sheet-body')).toBeTruthy()
    expect(container.querySelector('.fox-provider-dialog-body')).toBeNull()
    expect(screen.queryByRole('combobox', { name: '服务模板' })).toBeNull()
    expect(screen.getByRole('button', { name: '保存' })).toBeTruthy()
    expect(screen.getByRole('button', { name: '删除服务' })).toBeTruthy()
  })
})
