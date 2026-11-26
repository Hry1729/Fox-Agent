import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

import type { LifecycleHookRecord } from '../src/features/conversations/model/types'
import type { PluginCardView, PluginGateway } from '../src/features/plugins/model'

const hookRow = (overrides: Partial<LifecycleHookRecord>): LifecycleHookRecord => ({
  id: 'hook-1',
  name: '高风险工具审批',
  event: 'before_tool',
  matcher: 'run_command',
  action: 'require_approval',
  reason: '命令需要人工确认',
  enabled: true,
  priority: 100,
  createdAt: 1,
  updatedAt: 1,
  ...overrides,
})

let hooks: LifecycleHookRecord[] = []
const deleted: string[] = []
const toggled: Array<[string, boolean]> = []

// Spread the real modules: a `mock.module` factory replaces the module for the
// whole process, and a partial factory would break other test files.
const actualDesktopClient = { ...await import('../src/features/conversations/api/desktop-client') }
const actualNotifications = { ...await import('../src/features/notifications') }

afterAll(() => {
  mock.module('../src/features/conversations/api/desktop-client', () => actualDesktopClient)
  mock.module('../src/features/notifications', () => actualNotifications)
})

mock.module('../src/features/conversations/api/desktop-client', () => ({
  ...actualDesktopClient,
  desktopRuntimeAvailable: true,
  desktopClient: {
    ...actualDesktopClient.desktopClient,
    listLifecycleHooks: async () => hooks.map((hook) => ({ ...hook })),
    deleteLifecycleHook: async (hookId: string) => {
      deleted.push(hookId)
      hooks = hooks.filter((hook) => hook.id !== hookId)
      return true
    },
    setLifecycleHookEnabled: async (hookId: string, enabled: boolean) => {
      toggled.push([hookId, enabled])
      const hook = hooks.find((item) => item.id === hookId)!
      hook.enabled = enabled
      return { ...hook }
    },
  },
}))

mock.module('../src/features/notifications', () => ({
  ...actualNotifications,
  notify: Object.assign(() => {}, { success: () => {}, error: () => {}, info: () => {}, warning: () => {} }),
}))

const React = await import('react')
const { act, cleanup, fireEvent, render, waitFor } = await import('@testing-library/react')
const { PluginCenterPage } = await import('../src/features/plugins/plugin-center-page')
const { default: PluginDetailPage } = await import('../src/features/plugins/plugin-detail-page')
const { ConfirmDialogProvider, setConfirmDialogPresenter } = await import('../src/components/ui/confirm-dialog')

const gateway: PluginGateway = {
  catalogList: async () => ({ items: [], categories: [], total: 0 } as any),
  installationsList: async () => ({ items: [] } as any),
  operationsList: async () => [],
  install: async () => { throw new Error('unused') },
  uninstall: async () => { throw new Error('unused') },
  update: async () => { throw new Error('unused') },
  setActivation: async () => { throw new Error('unused') },
  agentScopeList: async () => [],
}

afterEach(() => {
  cleanup()
  setConfirmDialogPresenter(null)
  deleted.length = 0
  toggled.length = 0
})

// Plain DOM lookups: role queries are slow in happy-dom.
const tabTriggers = () => [...document.querySelectorAll('[data-slot="tabs-trigger"]')] as HTMLElement[]
const hookCards = () => [...document.querySelectorAll('[data-hook-id]')] as HTMLElement[]
const hookCard = (id: string) => {
  const found = document.querySelector(`[data-hook-id="${id}"]`)
  if (!found) throw new Error(`no hook card ${id}`)
  return found as HTMLElement
}
const dialog = () => document.querySelector('[role="alertdialog"]') as HTMLElement | null
const dialogButton = (name: string) => {
  const found = [...(dialog()?.querySelectorAll('button') ?? [])].find((item) => item.textContent?.trim() === name)
  if (!found) throw new Error(`no dialog button ${name}`)
  return found as HTMLElement
}

async function mount(props: Partial<React.ComponentProps<typeof PluginCenterPage>> = {}) {
  await act(async () => {
    render(React.createElement(React.Fragment, null,
      React.createElement(ConfirmDialogProvider),
      React.createElement(PluginCenterPage, { gateway, ...props }),
    ))
  })
}

async function openPolicy() {
  const policy = [...document.querySelectorAll('button')].find(button => button.textContent?.trim() === '插件策略')!
  await act(async () => { fireEvent.click(policy) })
  await waitFor(() => { if (hookCards().length === 0) throw new Error('hooks not listed yet') })
}

describe('插件中心的策略抽屉', () => {
  test('目录只显示连接器、插件和技能标签', async () => {
    hooks = [hookRow({})]
    await mount()
    const labels = tabTriggers().map((item) => item.textContent?.trim())
    expect(labels).toEqual(['连接器', '插件', '技能'])
    expect(hookCards()).toHaveLength(0)
  })

  test('目录通过真实视图映射打开搜索插件详情', async () => {
    const calls: unknown[][] = []
    const card = { id: 'search:exa', kind: 'tool', origin: 'builtin', category: '搜索服务', name: 'Exa', description: '搜索网页',
      installStatus: 'installed', compatible: true, permissions: [], activation: { mode: 'global', enabled: false } } as const
    await mount({ initialKind: 'plugin', navigate: (...args) => calls.push(args), gateway: {
      ...gateway, catalogList: async () => ({ items: [{ ...card, permissions: [] }], categories: ['搜索服务'], total: 1 }),
    } })
    const entry = await waitFor(() => {
      const result = document.querySelector<HTMLButtonElement>('[data-plugin-id="search:exa"]')
      if (!result) throw new Error('搜索插件未显示')
      return result
    })
    fireEvent.click(entry)
    expect(calls).toEqual([['plugin-detail', 'search:exa', { documentId: 'plugin' }]])
  })

  test.each(['mcp', 'plugin', 'skill'] as const)('%s 分类默认展示六项，可以独立展开和收起', async section => {
    const items = Array.from({ length: 15 }, (_, index) => ({
      id: (section === 'plugin' ? 'search' : section) + ':' + index,
      kind: section === 'plugin' ? 'tool' as const : section,
      name: '条目 ' + index, category: index < 8 ? 'A' : 'B', description: '测试条目',
      origin: 'builtin' as const, compatible: true, permissions: [], installStatus: 'not_installed' as const,
      activation: { mode: 'global' as const, enabled: false },
    }))
    await mount({ initialKind: section, gateway: { ...gateway, catalogList: async () => ({ items, categories: ['A', 'B'], total: items.length }) } })
    const groups = document.querySelectorAll('.fox-extension-group')
    const counts = () => Array.from(groups, group => group.querySelectorAll('[data-plugin-id]').length)
    expect(counts()).toEqual([6, 6])
    const toggle = groups[0].querySelector('header button')!
    fireEvent.click(toggle)
    expect(counts()).toEqual([8, 6])
    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    fireEvent.click(toggle)
    expect(counts()).toEqual([6, 6])
  })

  test('lists the lifecycle Hooks with their enable switch', async () => {
    hooks = [
      hookRow({}),
      hookRow({ id: 'hook-2', name: '附加说明', event: 'after_tool', matcher: 'call_*', action: 'annotate', enabled: false }),
    ]
    await mount()
    await openPolicy()
    expect(hookCards().map((card) => card.getAttribute('data-hook-id'))).toEqual(['hook-1', 'hook-2'])
    expect(hookCard('hook-1').textContent).toContain('高风险工具审批')
    expect(hookCard('hook-2').textContent).toContain('after_tool · call_* · annotate')
    expect(document.querySelector('[data-slot=sheet-content]')?.textContent).toContain('插件策略与 Hook')
    // The add / edit form is part of the same section.
    expect(document.body.textContent).toContain('添加 Hook')

    const toggle = hookCard('hook-2').querySelector('[role="switch"]') as HTMLElement
    expect(toggle).toBeTruthy()
    await act(async () => { fireEvent.click(toggle) })
    await waitFor(() => { if (toggled.length === 0) throw new Error('not toggled') })
    expect(toggled).toEqual([['hook-2', true]])
  })

  test('initialTab="policy" opens straight on the Hook list', async () => {
    hooks = [hookRow({})]
    await mount({ initialTab: 'policy' })
    await waitFor(() => { if (hookCards().length !== 1) throw new Error('not listed') })
    expect(document.querySelector('[data-slot=sheet-content]')?.textContent).toContain('插件策略与 Hook')
  })

  test('deleting a Hook asks first: cancel keeps it, confirm deletes it', async () => {
    hooks = [hookRow({})]
    await mount()
    await openPolicy()
    const deleteButton = () => hookCard('hook-1').querySelector('button[title="删除"]') as HTMLElement

    await act(async () => { fireEvent.click(deleteButton()) })
    await waitFor(() => { if (!dialog()) throw new Error('no confirmation') })
    expect(dialog()!.textContent).toContain('删除 Hook')
    expect(dialog()!.textContent).toContain('高风险工具审批')
    await act(async () => { fireEvent.click(dialogButton('取消')) })
    await waitFor(() => { if (dialog()) throw new Error('still open') })
    expect(deleted).toEqual([])
    expect(hookCards()).toHaveLength(1)

    await act(async () => { fireEvent.click(deleteButton()) })
    await waitFor(() => { if (!dialog()) throw new Error('no confirmation') })
    await act(async () => { fireEvent.click(dialogButton('删除')) })
    await waitFor(() => { if (hookCards().length !== 0) throw new Error('not removed') })
    expect(deleted).toEqual(['hook-1'])
  })
})

test('示例指令可填入新对话，立即试用在启用后显示', async () => {
  const card: PluginCardView = {
    id: 'search:exa', kind: 'tool', origin: 'builtin', category: '搜索服务', name: 'Exa',
    description: '按语义检索网页', compatible: true, permissions: [], installStatus: 'installed',
    activation: { mode: 'global', enabled: false },
  }
  const navigations: unknown[][] = []
  const setActivation = mock(async (id, update) => {
    card.activation = update
    return { ...card, id }
  })
  const view = render(React.createElement(PluginDetailPage, {
    id: card.id, kind: 'plugin', fromSettings: true,
    navigate: (...args) => navigations.push(args),
    gateway: { ...gateway, installationsList: async () => ({ items: [{ ...card }], total: 1 }), setActivation },
  }))
  const example = await view.findByRole<HTMLButtonElement>('button', { name: '使用 Exa 检索近期关于 MCP 协议更新的官方公告并总结要点' })
  expect(view.getByRole('region', { name: '示例指令' }).querySelectorAll('button')).toHaveLength(3)
  expect(view.queryByRole('button', { name: '立即试用' })).toBeNull()
  fireEvent.click(example)
  expect(navigations).toEqual([['chat', undefined, { draftPrompt: example.getAttribute('aria-label') }]])
  fireEvent.click(view.getByRole('button', { name: '选用' }))
  await view.findByRole('button', { name: '立即试用' })
  expect(setActivation).toHaveBeenCalledWith('search:exa', { mode: 'global', enabled: true })
})
