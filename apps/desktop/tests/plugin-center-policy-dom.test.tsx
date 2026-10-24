// sandbox.md 13.22: the plugin center has a fourth tab「策略」that lists the
// declarative lifecycle Hooks through the shared LifecycleHooksSection (the
// same component the connector settings page renders). Deleting a Hook still
// goes through the in-app confirmation: cancel keeps it, confirm deletes it.

import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

import type { LifecycleHookRecord } from '../src/features/conversations/model/types'
import type { PluginGateway } from '../src/features/plugins/model'

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
const tab = (label: string) => {
  const found = tabTriggers().find((item) => item.textContent?.trim() === label)
  if (!found) throw new Error(`no tab ${label}; tabs: ${tabTriggers().map((item) => item.textContent).join(',')}`)
  return found
}
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
  const trigger = tab('策略')
  await act(async () => {
    fireEvent.mouseDown(trigger, { button: 0 })
    fireEvent.click(trigger)
  })
  await waitFor(() => { if (hookCards().length === 0) throw new Error('hooks not listed yet') })
}

describe('plugin center「策略」tab', () => {
  test('is the fourth tab after the three plugin kinds', async () => {
    hooks = [hookRow({})]
    await mount()
    const labels = tabTriggers().map((item) => item.textContent?.trim())
    expect(labels).toHaveLength(4)
    expect(labels[3]).toBe('策略')
    // The plugin kinds stay first and the policy list is not shown yet.
    expect(hookCards()).toHaveLength(0)
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
    expect(tab('策略').getAttribute('data-state')).toBe('active')
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
    expect(tab('策略').getAttribute('data-state')).toBe('active')
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
