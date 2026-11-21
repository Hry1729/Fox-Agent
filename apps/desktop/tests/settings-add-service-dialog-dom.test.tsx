import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
const actual = { ...await import('../src/features/conversations/api/desktop-client') }
const calls: string[] = []
const savedInputs: Array<Parameters<typeof actual.desktopClient.saveModelProvider>[0]> = []
mock.module('../src/features/conversations/api/desktop-client', () => ({ ...actual, desktopRuntimeAvailable: true, desktopClient: { ...actual.desktopClient,
  listModelProviders: async () => [],
  agentConfigOverview: async () => { calls.push('overview'); return { services: [], agents: [] } },
  agentConfigVerify: async () => { calls.push('verify'); throw new Error('unexpected verification') },
  testModelService: async () => { calls.push('test'); return { latencyMs: 10, models: ['gpt-4.1', 'gpt-4.1-mini'] } },
  saveModelProvider: async (input: Parameters<typeof actual.desktopClient.saveModelProvider>[0]) => {
    calls.push('save'); savedInputs.push(input); throw new Error('保存测试结束')
  },
} }))
afterAll(() => mock.module('../src/features/conversations/api/desktop-client', () => actual))
const React = await import('react')
const { act, cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const { ModelProvidersPage } = await import('../src/features/settings/settings-pages')
const { ServiceForm } = await import('../src/features/settings/model-agent/service-form')
const { serviceDraft } = await import('../src/features/settings/model-agent/service-presets')
const { TooltipProvider } = await import('../src/components/ui/tooltip')
const { MotionGlobalConfig } = await import('motion/react')
const previousMotion = MotionGlobalConfig.skipAnimations
MotionGlobalConfig.skipAnimations = true
afterAll(() => { MotionGlobalConfig.skipAnimations = previousMotion })
afterEach(() => { cleanup(); calls.length = 0; savedInputs.length = 0 })
const page = () => render(React.createElement(TooltipProvider, null, React.createElement(ModelProvidersPage, { sidebarCollapsed: false, onSidebar: () => {} })))
const sheet = () => document.querySelector<HTMLElement>('[data-slot="sheet-content"]')!
const sheetButton = (text: string) => [...sheet().querySelectorAll('button')].find(button => button.textContent === text)!

describe('model provider settings sheet', () => {
  test('opening and focusing reads overview without saving or verifying', async () => {
    await act(async () => { page() })
    await waitFor(() => expect(calls).toEqual(['overview']))
    await act(async () => { window.dispatchEvent(new Event('focus')) })
    expect(calls).toEqual(['overview', 'overview'])
  })
  test('searching opens the preset in the animated right sheet and returning preserves search', async () => {
    await act(async () => { page() })
    fireEvent.click(screen.getByRole('button', { name: '添加模型' }))
    expect(sheet().getAttribute('data-side')).toBe('right')
    const search = () => sheet().querySelector<HTMLInputElement>('[aria-label="搜索供应商"]')!
    fireEvent.change(search(), { target: { value: 'api.openai.com' } })
    expect(sheetButton('Anthropic')).toBeUndefined()
    fireEvent.click(sheetButton('OpenAI'))
    expect(sheet().textContent).toContain('https://api.openai.com/v1')
    expect(sheet().querySelector('input[type="password"]')).not.toBeNull()
    fireEvent.click(sheetButton('供应商列表'))
    expect(search().value).toBe('api.openai.com')
    fireEvent.change(search(), { target: { value: 'Aliyun Coding Plan (International)' } })
    const international = sheetButton('Aliyun · 国际')
    expect(international.getAttribute('aria-label')).toBe('Aliyun Coding Plan (International)')
    expect(sheet().textContent).not.toContain('使用国际站 Coding Plan')
    fireEvent.click(international)
    expect(sheet().querySelector('[data-provider-note]')?.textContent).toContain('使用国际站 Coding Plan')
    fireEvent.click(sheetButton('供应商列表'))
    fireEvent.click(sheetButton('自定义供应商'))
    expect(sheet().textContent).toContain('服务名称')
    expect(sheetButton('更多端点').getAttribute('aria-expanded')).toBe('false')
    expect(sheetButton('高级设置').getAttribute('aria-expanded')).toBe('false')
    await act(async () => { fireEvent.click(sheetButton('取消')) })
    expect(calls.every(call => call === 'overview')).toBe(true)
  })
  test('a successful preset test supplies a real default model to the save call', async () => {
    await act(async () => { page() })
    fireEvent.click(screen.getByRole('button', { name: '添加模型' }))
    fireEvent.click(sheetButton('OpenAI'))
    fireEvent.change(screen.getByLabelText('API Key'), { target: { value: 'test-only' } })
    expect(sheetButton('添加').disabled).toBe(true)
    await act(async () => { fireEvent.click(sheetButton('检查连接')) })
    expect(sheet().textContent).toContain('默认模型 gpt-4.1')
    fireEvent.click(sheetButton('添加'))
    await waitFor(() => expect(savedInputs[0]?.models[0]?.modelId).toBe('gpt-4.1'))
  })
  test('custom provider saves multiple models and the additional protocol address', async () => {
    await act(async () => { page() })
    fireEvent.click(screen.getByRole('button', { name: '添加模型' }))
    fireEvent.click(sheetButton('自定义供应商'))
    const primaryInputs = sheet().querySelectorAll<HTMLInputElement>('.fox-provider-edit-fields > section input')
    fireEvent.change(primaryInputs[0], { target: { value: '测试网关' } })
    fireEvent.change(primaryInputs[1], { target: { value: 'https://relay.example.com/v1' } })
    fireEvent.change(primaryInputs[3], { target: { value: 'gpt-4.1，gpt-4.1-mini, gpt-4.1' } })
    const endpoints = sheetButton('更多端点')
    fireEvent.click(endpoints)
    expect(endpoints.getAttribute('aria-expanded')).toBe('true')
    fireEvent.click(sheetButton('添加协议'))
    const extraAddress = screen.getByPlaceholderText<HTMLInputElement>('填写供应商提供的此协议地址')
    fireEvent.change(extraAddress, { target: { value: 'https://relay.example.com/responses' } })
    expect(endpoints.textContent).toContain('1 个端点')
    fireEvent.click(endpoints)
    expect(endpoints.getAttribute('aria-expanded')).toBe('false')
    fireEvent.click(endpoints)
    expect(extraAddress.value).toBe('https://relay.example.com/responses')
    await act(async () => { fireEvent.click(sheetButton('检查连接')) })
    await act(async () => { fireEvent.click(sheetButton('添加')) })
    expect(savedInputs[0]?.models.map(model => model.modelId)).toEqual(['gpt-4.1', 'gpt-4.1-mini'])
    expect(savedInputs[0]?.protocolProfiles?.[0]).toMatchObject({ apiType: 'openai-responses', baseUrl: 'https://relay.example.com/responses' })
  })
  test('editing a preset exposes its connection fields and preserves retained model metadata', () => {
    const draft = { ...serviceDraft(), id: 'p1', name: 'Example', baseUrl: 'https://api.openai.com/v1', models: [{ id: 'm1', modelId: 'model', displayName: 'Model', contextWindow: 8192, maxOutputTokens: 1024, supportsImageInput: false, isDefault: true }] }
    const onChange = mock(() => {})
    const result = render(React.createElement(ServiceForm, { form: draft, targets: ['fox'], enabled: [], existingServices: [], busy: false, credentialConfigured: true, error: null, onChange, onTargets: () => {}, onSave: () => {}, onCancel: () => {}, onDelete: () => {} }))
    expect(result.container.querySelector('.fox-service-form')).toBeTruthy()
    expect(result.container.querySelector('[role="dialog"]')).toBeNull()
    expect(screen.getByRole('button', { name: '保存' })).toBeTruthy()
    expect(screen.getByRole('button', { name: '删除服务' })).toBeTruthy()
    expect((screen.getByLabelText('API 地址') as HTMLInputElement).value).toBe(draft.baseUrl)
    fireEvent.change(screen.getByPlaceholderText('多个 ID 用逗号分隔'), { target: { value: 'new-model, model' } })
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({
      models: [expect.objectContaining({ modelId: 'new-model', isDefault: false }), draft.models[0]],
    }))
    const advanced = screen.getByRole('button', { name: '高级设置' })
    expect(screen.queryByLabelText('上下文长度')).toBeNull()
    fireEvent.click(advanced)
    expect(advanced.getAttribute('aria-expanded')).toBe('true')
    expect(screen.getByLabelText('上下文长度')).toBeTruthy()
    expect(screen.getByRole('group', { name: '模型 1' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: '删除模型' })).toBeNull()
    expect(screen.getByRole('switch', { name: '设为默认服务' })).toBeTruthy()
  })
})
