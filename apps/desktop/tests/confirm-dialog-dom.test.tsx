// The shared in-app confirmation (`src/components/ui/confirm-dialog.tsx`):
// Promise<boolean>, Chinese title, destructive confirm button, cancel / Escape /
// no provider / unmount all answer false, requests are shown one at a time.

import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { act, cleanup, fireEvent, render, waitFor } = await import('@testing-library/react')
const { ConfirmDialogProvider, confirmDialog, setConfirmDialogPresenter } = await import(
  '../src/components/ui/confirm-dialog'
)

afterEach(() => {
  cleanup()
  setConfirmDialogPresenter(null)
})

// Plain DOM lookups: role queries walk the computed accessibility tree and are
// slow enough in happy-dom to hit the per-test timeout.
const dialogs = () => [...document.querySelectorAll('[role="alertdialog"]')]
const theDialog = () => {
  const found = dialogs()
  if (found.length !== 1) throw new Error(`expected one alertdialog, found ${found.length}`)
  return found[0] as HTMLElement
}
const button = (name: string) => {
  const found = [...theDialog().querySelectorAll('button')].find((item) => item.textContent?.trim() === name)
  if (!found) throw new Error(`no button ${name}`)
  return found as HTMLElement
}

const mountProvider = () => act(async () => { render(React.createElement(ConfirmDialogProvider)) })

/** Ask, then wait until the dialog with that title is on screen. */
async function ask(options: Parameters<typeof confirmDialog>[0]) {
  let answer!: Promise<boolean>
  await act(async () => { answer = confirmDialog(options) })
  await waitFor(() => theDialog())
  // Wrapped: returning the bare promise from an async function would adopt it
  // and wait for the answer itself.
  return { answer }
}

describe('confirmDialog', () => {
  test('without a provider it answers false and never throws', async () => {
    const original = console.warn
    console.warn = () => {}
    try {
      expect(await confirmDialog({ title: '删除服务' })).toBe(false)
    } finally {
      console.warn = original
    }
  })

  test('a stub presenter decides the answer; a rejecting one counts as cancel', async () => {
    const seen: string[] = []
    const remove = setConfirmDialogPresenter(async (options) => { seen.push(options.title); return true })
    expect(await confirmDialog({ title: '删除 Hook' })).toBe(true)
    remove()
    setConfirmDialogPresenter(async () => { throw new Error('boom') })
    expect(await confirmDialog({ title: '删除 Hook' })).toBe(false)
    expect(seen).toEqual(['删除 Hook'])
  })

  test('shows the Chinese title and description, confirm resolves true', async () => {
    await mountProvider()
    const { answer } = await ask({ title: '删除服务', description: '删除服务“DeepSeek”？', confirmLabel: '删除', destructive: true })
    const dialog = theDialog()
    expect(dialog.textContent).toContain('删除服务')
    expect(dialog.textContent).toContain('删除服务“DeepSeek”？')
    expect(dialog.textContent).not.toContain('127.0.0.1')
    const confirm = button('删除')
    // Destructive confirm button uses the destructive variant.
    expect(confirm.className).toContain('text-destructive')
    await act(async () => { fireEvent.click(confirm) })
    expect(await answer).toBe(true)
    await waitFor(() => expect(dialogs()).toHaveLength(0))
  })

  test('cancel resolves false; the default labels are 确定 / 取消', async () => {
    await mountProvider()
    const { answer } = await ask({ title: '恢复专家版本' })
    const confirm = button('确定')
    expect(confirm.className).not.toContain('text-destructive')
    await act(async () => { fireEvent.click(button('取消')) })
    expect(await answer).toBe(false)
  })

  test('Escape resolves false', async () => {
    await mountProvider()
    const { answer } = await ask({ title: '放弃修改', confirmLabel: '放弃修改', cancelLabel: '继续编辑', destructive: true })
    await act(async () => { fireEvent.keyDown(theDialog(), { key: 'Escape' }) })
    expect(await answer).toBe(false)
  })

  test('requests are shown one at a time, in order', async () => {
    await mountProvider()
    const { answer: first } = await ask({ title: '第一个', confirmLabel: '好' })
    let second!: Promise<boolean>
    await act(async () => { second = confirmDialog({ title: '第二个', confirmLabel: '好' }) })
    expect(dialogs()).toHaveLength(1)
    expect(theDialog().textContent).toContain('第一个')
    await act(async () => { fireEvent.click(button('好')) })
    expect(await first).toBe(true)
    await waitFor(() => expect(theDialog().textContent).toContain('第二个'))
    await act(async () => { fireEvent.click(button('取消')) })
    expect(await second).toBe(false)
  })

  test('unmounting the provider answers an open request with false', async () => {
    await mountProvider()
    const { answer } = await ask({ title: '清理预览缓存' })
    await act(async () => { cleanup() })
    expect(await answer).toBe(false)
  })
})
