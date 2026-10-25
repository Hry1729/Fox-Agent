// 模型 settings, service row「⋯」menu: the in-app confirmation must open every
// time 删除 is chosen, including the second time the menu is opened (the menu's
// close/focus restore used to race the confirm dialog). Confirming deletes the
// service; the same deferral applies to 重命名 and its prompt dialog.
import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const provider = {
  id: 'svc-test', name: '测试服务', icon: null, baseUrl: 'https://test-provider.example.com/v1', apiType: 'openai-completions',
  isDefault: false, credentialConfigured: true, lastStatus: 'connected', lastLatencyMs: 10, lastCheckedAt: 1, lastError: null,
  protocolProfiles: [], createdAt: 1, updatedAt: 1,
  models: [{ id: 'm1', modelId: 'test-model', displayName: 'Test', contextWindow: 8192, maxOutputTokens: 1024, supportsImageInput: false, isDefault: true }],
}
let providers: any[] = []
const deleted: string[] = []

const actualDesktopClient = { ...await import('../src/features/conversations/api/desktop-client') }
afterAll(() => { mock.module('../src/features/conversations/api/desktop-client', () => actualDesktopClient) })
mock.module('../src/features/conversations/api/desktop-client', () => ({
  ...actualDesktopClient,
  desktopRuntimeAvailable: true,
  desktopClient: {
    ...actualDesktopClient.desktopClient,
    listModelProviders: async () => providers.map((item) => ({ ...item })),
    deleteModelProvider: async (id: string) => { deleted.push(id); providers = providers.filter((item) => item.id !== id); return true },
    getModelServiceStatus: async () => null,
  },
}))

const React = await import('react')
const { act, cleanup, fireEvent, render, waitFor } = await import('@testing-library/react')
const { ModelProvidersPage } = await import('../src/features/settings/settings-pages')
const { TooltipProvider } = await import('../src/components/ui/tooltip')
const { ConfirmDialogProvider } = await import('../src/components/ui/confirm-dialog')
const { PromptDialogProvider } = await import('../src/components/ui/prompt-dialog')

afterEach(() => { cleanup(); deleted.length = 0 })

const confirmBox = () => document.querySelector('[data-testid="fox-confirm-dialog"]') as HTMLElement | null
const buttonIn = (root: ParentNode, name: string) => {
  const found = [...root.querySelectorAll('button')].find((item) => item.textContent?.trim() === name)
  if (!found) throw new Error(`no button ${name}`)
  return found as HTMLElement
}
const menuItem = (name: string) => {
  const found = [...document.querySelectorAll('[role="menuitem"]')].find((item) => item.textContent?.trim() === name)
  if (!found) throw new Error(`no menu item ${name}`)
  return found as HTMLElement
}

async function mount() {
  await act(async () => {
    render(React.createElement(TooltipProvider, null,
      React.createElement(ConfirmDialogProvider),
      React.createElement(PromptDialogProvider),
      React.createElement(ModelProvidersPage, { sidebarCollapsed: false, onSidebar: () => {} })))
  })
  await waitFor(() => { if (!document.querySelector('[aria-label="管理 测试服务"]')) throw new Error('row not rendered') })
}

async function openMenuAndChoose(name: string) {
  const trigger = document.querySelector('[aria-label="管理 测试服务"]') as HTMLElement
  await act(async () => { fireEvent.keyDown(trigger, { key: 'Enter' }) })
  await waitFor(() => menuItem(name))
  await act(async () => { fireEvent.click(menuItem(name)) })
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 30)) })
}

describe('service row ⋯ menu', () => {
  test('删除 opens the confirm dialog on the first and the second menu open; confirming deletes', async () => {
    providers = [{ ...provider }]
    await mount()

    await openMenuAndChoose('删除')
    await waitFor(() => { if (!confirmBox()) throw new Error('first confirm missing') })
    expect(confirmBox()!.textContent).toContain('删除服务')
    await act(async () => { fireEvent.click(buttonIn(confirmBox()!, '取消')) })
    await waitFor(() => { if (confirmBox()) throw new Error('confirm still open') })
    expect(deleted).toEqual([])

    await openMenuAndChoose('删除')
    await waitFor(() => { if (!confirmBox()) throw new Error('second confirm missing') })
    await act(async () => { fireEvent.click(buttonIn(confirmBox()!, '删除')) })
    await waitFor(() => expect(deleted).toEqual(['svc-test']))
  })

  test('重命名 opens the prompt dialog on repeated menu opens', async () => {
    providers = [{ ...provider }]
    await mount()
    for (let round = 0; round < 2; round += 1) {
      await openMenuAndChoose('重命名')
      await waitFor(() => { if (!document.querySelector('[role="dialog"], [role="alertdialog"]')) throw new Error(`prompt missing (round ${round + 1})`) })
      const box = document.querySelector('[role="dialog"], [role="alertdialog"]') as HTMLElement
      await act(async () => { fireEvent.click(buttonIn(box, '取消')) })
      await waitFor(() => { if (document.querySelector('[role="dialog"], [role="alertdialog"]')) throw new Error('prompt still open') })
    }
  })
})
