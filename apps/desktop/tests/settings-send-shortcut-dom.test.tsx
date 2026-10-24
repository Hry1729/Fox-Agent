// The 对话 settings page drives the real 发送方式 preference: choosing a mode
// persists it under `fox.preferences.sendKey`, and the hint under the control
// names the key that now breaks the line.
import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const { ConversationSettingsPage } = await import('../src/features/settings/settings-pages')
const { TooltipProvider } = await import('../src/components/ui/tooltip')
const { PROMPT_SEND_SHORTCUT_KEY, readPromptSendShortcut, resolvePromptEnterAction } = await import('../src/components/ai-elements/prompt-send-shortcut')

afterEach(() => {
  cleanup()
  window.localStorage.clear()
})

function mount() {
  return render(React.createElement(TooltipProvider, null, React.createElement(ConversationSettingsPage, { sidebarCollapsed: false, onSidebar: () => {} })))
}

/** Radix tabs activate on mousedown (pointer) or on keyboard focus. */
function choose(tab: HTMLElement) {
  fireEvent.mouseDown(tab, { button: 0 })
  fireEvent.click(tab)
}

describe('对话 settings: 发送方式', () => {
  test('defaults to Enter sending, with Shift + Enter as the newline hint', () => {
    mount()
    expect(screen.getByRole('tab', { name: 'Enter 发送' }).getAttribute('aria-selected')).toBe('true')
    expect(screen.getByTestId('send-shortcut-hint').textContent).toContain('Shift')
  })

  test('choosing Ctrl/Cmd + Enter persists it and changes what the composer keys do', async () => {
    mount()
    choose(screen.getByRole('tab', { name: 'Ctrl/Cmd + Enter 发送' }))
    await waitFor(() => expect(window.localStorage.getItem(PROMPT_SEND_SHORTCUT_KEY)).toBe('mod-enter'))
    expect(screen.getByRole('tab', { name: 'Ctrl/Cmd + Enter 发送' }).getAttribute('aria-selected')).toBe('true')
    expect(screen.getByTestId('send-shortcut-hint').textContent).not.toContain('Shift')
    const mode = readPromptSendShortcut()
    expect(resolvePromptEnterAction({ key: 'Enter' }, mode)).toBe('newline')
    expect(resolvePromptEnterAction({ key: 'Enter', ctrlKey: true }, mode)).toBe('send')
    expect(resolvePromptEnterAction({ key: 'Enter', ctrlKey: true, isComposing: true }, mode)).toBe('default')
  })

  test('a stored choice is shown again after reopening the page', () => {
    window.localStorage.setItem(PROMPT_SEND_SHORTCUT_KEY, 'mod-enter')
    mount()
    expect(screen.getByRole('tab', { name: 'Ctrl/Cmd + Enter 发送' }).getAttribute('aria-selected')).toBe('true')
  })

  test('the read-only 文件附件 row and the unbacked 消息导航 row are gone; 快捷键 is merged in', () => {
    mount()
    expect(screen.queryByText('文件附件')).toBeNull()
    expect(screen.queryByText('消息导航')).toBeNull()
    expect(screen.getByRole('region', { name: '快捷键' })).toBeTruthy()
    expect(screen.getByRole('button', { name: /恢复默认/ })).toBeTruthy()
  })
})
