import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
const actual = { ...await import('../src/features/conversations/api/desktop-client') }
const calls: string[] = []
mock.module('../src/features/conversations/api/desktop-client', () => ({ ...actual, desktopRuntimeAvailable: true, desktopClient: { ...actual.desktopClient,
  listModelProviders: async () => [],
  agentConfigOverview: async () => { calls.push('overview'); return { services: [], agents: [] } },
  agentConfigVerify: async () => { calls.push('verify'); throw new Error('unexpected verification') },
  saveModelProvider: async () => { calls.push('save'); throw new Error('unexpected save') },
} }))
afterAll(() => mock.module('../src/features/conversations/api/desktop-client', () => actual))
const React = await import('react')
const { act, cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const { ModelProvidersPage } = await import('../src/features/settings/settings-pages')
const { ServiceForm } = await import('../src/features/settings/model-agent/service-form')
const { serviceDraft } = await import('../src/features/settings/model-agent/service-presets')
const { TooltipProvider } = await import('../src/components/ui/tooltip')
afterEach(() => { cleanup(); calls.length = 0 })
const page = () => render(React.createElement(TooltipProvider, null, React.createElement(ModelProvidersPage, { sidebarCollapsed: false, onSidebar: () => {} })))

describe('model and agent inline settings', () => {
  test('opening and focusing reads overview without saving or verifying', async () => {
    await act(async () => { page() })
    await waitFor(() => expect(calls).toEqual(['overview']))
    await act(async () => { window.dispatchEvent(new Event('focus')) })
    expect(calls).toEqual(['overview', 'overview'])
  })
  test('adding uses targets then required fields in the page; cancel returns to the list', async () => {
    await act(async () => { page() })
    fireEvent.click(screen.getByRole('button', { name: '添加服务' }))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(screen.getByText('这个服务给谁用')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: '下一步' }))
    expect(screen.getByLabelText('服务名称')).toBeTruthy()
    expect(screen.getByLabelText('默认模型')).toBeTruthy()
    expect(screen.getByRole('button', { name: '测试连接' })).toBeTruthy()
    expect(screen.getByText(/高级设置/).closest('details')?.open).toBe(false)
    fireEvent.click(screen.getAllByRole('button', { name: '取消' })[0])
    expect(screen.getByRole('button', { name: '添加服务' })).toBeTruthy()
    expect(calls.every(call => call === 'overview')).toBe(true)
  })
  test('editing uses the same inline form and keeps model fields editable', () => {
    const draft = { ...serviceDraft(), id: 'p1', name: 'Example', baseUrl: 'https://example.com/v1', models: [{ modelId: 'model', displayName: 'Model', contextWindow: 8192, maxOutputTokens: 1024, supportsImageInput: false, isDefault: true }] }
    const result = render(React.createElement(ServiceForm, { form: draft, targets: ['fox'], enabled: [], existingServices: [], busy: false, credentialConfigured: true, error: null, onChange: () => {}, onTargets: () => {}, onSave: () => {}, onCancel: () => {}, onDelete: () => {} }))
    expect(result.container.querySelector('.fox-service-form')).toBeTruthy()
    expect(result.container.querySelector('[role="dialog"]')).toBeNull()
    expect(screen.getByRole('button', { name: '保存' })).toBeTruthy()
    expect(screen.getByRole('button', { name: '删除服务' })).toBeTruthy()
    fireEvent.click(screen.getByText(/高级设置/))
    expect(screen.getByLabelText('上下文长度')).toBeTruthy()
  })
})
