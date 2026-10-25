// 设置 → 数据与关于 → 预览缓存 →「清理未使用缓存」: with a non-zero clearable
// preview cache the button is enabled, a click opens the in-app alertdialog
// (never the native confirm box), cancelling clears nothing, and the cleanup
// runs only after 「清理」 is confirmed.
import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const MiB = 1024 * 1024
const calls: string[] = []
let cache = { totalFiles: 4, activeFiles: 1, totalBytes: 6 * MiB, activeBytes: 1 * MiB, limitBytes: 1024 * MiB }

const actualDesktopClient = { ...await import('../src/features/conversations/api/desktop-client') }
afterAll(() => { mock.module('../src/features/conversations/api/desktop-client', () => actualDesktopClient) })
mock.module('../src/features/conversations/api/desktop-client', () => ({
  ...actualDesktopClient,
  desktopRuntimeAvailable: true,
  desktopClient: {
    ...actualDesktopClient.desktopClient,
    listMcpServers: async () => [],
    listManagedProjects: async () => [],
    listModelProviders: async () => [],
    usageStatistics: async () => { throw new Error('not in this test') },
    observabilityStatistics: async () => { throw new Error('not in this test') },
    runtimeDiagnostics: async () => { throw new Error('not in this test') },
    knowledgePreviewCacheStatistics: async () => { calls.push('stats'); return { ...cache } },
    clearKnowledgePreviewCache: async () => {
      calls.push('clear-cache')
      const bytes = cache.totalBytes - cache.activeBytes
      cache = { ...cache, totalFiles: cache.activeFiles, totalBytes: cache.activeBytes }
      return { message: '已清理预览缓存', bytes }
    },
  },
}))

const React = await import('react')
const { act, cleanup, fireEvent, render, waitFor } = await import('@testing-library/react')
const { DataAboutSettingsPage } = await import('../src/features/settings/settings-data-page')
const { TooltipProvider } = await import('../src/components/ui/tooltip')
const { ConfirmDialogProvider } = await import('../src/components/ui/confirm-dialog')

const nativeConfirm = window.confirm
afterEach(() => {
  cleanup()
  calls.length = 0
  window.confirm = nativeConfirm
  cache = { totalFiles: 4, activeFiles: 1, totalBytes: 6 * MiB, activeBytes: 1 * MiB, limitBytes: 1024 * MiB }
})

const buttonNamed = (root: ParentNode, name: string) =>
  [...root.querySelectorAll('button')].find((item) => item.textContent?.trim() === name) as HTMLButtonElement | undefined
const alertDialog = () => document.querySelector('[role="alertdialog"]') as HTMLElement | null
const clearCalls = () => calls.filter((item) => item === 'clear-cache')

async function mountReady() {
  const nativeCalls: unknown[] = []
  window.confirm = ((message?: string) => { nativeCalls.push(message); return true }) as typeof window.confirm
  await act(async () => {
    render(React.createElement(TooltipProvider, null,
      React.createElement(ConfirmDialogProvider),
      React.createElement(DataAboutSettingsPage, { sidebarCollapsed: false, onSidebar: () => {}, navigate: () => {} })))
  })
  const button = await waitFor(() => {
    const found = buttonNamed(document, '清理未使用缓存')
    if (!found || found.disabled) throw new Error('清理未使用缓存 not enabled yet')
    return found
  })
  expect(document.body.textContent).toContain('可清理 5')
  return { button, nativeCalls }
}

describe('数据与关于 → 预览缓存 → 清理未使用缓存', () => {
  test('opens the in-app alertdialog; cancelling clears nothing', async () => {
    const { button, nativeCalls } = await mountReady()
    expect(alertDialog()).toBeNull()
    await act(async () => { fireEvent.click(button) })
    const dialog = await waitFor(() => { const box = alertDialog(); if (!box) throw new Error('no alertdialog'); return box })
    expect(dialog.getAttribute('data-testid')).toBe('fox-confirm-dialog')
    expect(dialog.textContent).toContain('清理预览缓存')
    expect(dialog.textContent).toContain('正在打开的文件会保留')
    expect(buttonNamed(dialog, '清理')).toBeTruthy()
    expect(nativeCalls).toEqual([])
    expect(clearCalls()).toEqual([])
    await act(async () => { fireEvent.click(buttonNamed(dialog, '取消')!) })
    await waitFor(() => { if (alertDialog()) throw new Error('dialog still open') })
    await new Promise((resolve) => setTimeout(resolve, 20))
    expect(clearCalls()).toEqual([])
    expect(buttonNamed(document, '清理未使用缓存')!.disabled).toBe(false)
  })

  test('cleanup runs once, only after 清理 is confirmed, then the usage refreshes', async () => {
    const { button, nativeCalls } = await mountReady()
    await act(async () => { fireEvent.click(button) })
    const dialog = await waitFor(() => { const box = alertDialog(); if (!box) throw new Error('no alertdialog'); return box })
    expect(clearCalls()).toEqual([])
    await act(async () => { fireEvent.click(buttonNamed(dialog, '清理')!) })
    await waitFor(() => expect(clearCalls()).toEqual(['clear-cache']))
    await waitFor(() => { if (alertDialog()) throw new Error('dialog still open') })
    // Refreshed statistics show nothing left to clear, so the button disables.
    await waitFor(() => { if (!buttonNamed(document, '清理未使用缓存')!.disabled) throw new Error('not refreshed') })
    expect(calls.indexOf('clear-cache')).toBeLessThan(calls.lastIndexOf('stats'))
    expect(nativeCalls).toEqual([])
  })
})
