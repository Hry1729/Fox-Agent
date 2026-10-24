// The shared in-app text prompt (`src/components/ui/prompt-dialog.tsx`), used by
// 「重命名服务」 in the model services settings: Chinese title (not the dev server
// address), prefilled value, Enter confirms, Escape / cancel / no provider /
// unmount answer null, an empty or whitespace-only value cannot be confirmed.

import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { act, cleanup, fireEvent, render, waitFor } = await import('@testing-library/react')
const { PromptDialogProvider, promptDialog, setPromptDialogPresenter } = await import(
  '../src/components/ui/prompt-dialog'
)

afterEach(() => {
  cleanup()
  setPromptDialogPresenter(null)
})

// Plain DOM lookups: role queries walk the computed accessibility tree and are
// slow in happy-dom.
const theDialog = () => {
  const found = [...document.querySelectorAll('[data-testid="fox-prompt-dialog"]')]
  if (found.length !== 1) throw new Error(`expected one prompt dialog, found ${found.length}`)
  return found[0] as HTMLElement
}
const input = () => theDialog().querySelector('input') as HTMLInputElement
const button = (name: string) => {
  const found = [...theDialog().querySelectorAll('button')].find((item) => item.textContent?.trim() === name)
  if (!found) throw new Error(`no button ${name}`)
  return found as HTMLButtonElement
}
const noDialog = () => document.querySelectorAll('[data-testid="fox-prompt-dialog"]').length === 0

const mountProvider = () => act(async () => { render(React.createElement(PromptDialogProvider)) })

const RENAME = { title: '重命名服务', label: '服务名称', defaultValue: 'DeepSeek', confirmLabel: '保存' }

async function ask(options: Parameters<typeof promptDialog>[0] = RENAME) {
  let answer!: Promise<string | null>
  await act(async () => { answer = promptDialog(options) })
  await waitFor(() => theDialog())
  return { answer }
}

describe('promptDialog', () => {
  test('without a provider it answers null and never throws', async () => {
    const original = console.warn
    console.warn = () => {}
    try {
      expect(await promptDialog(RENAME)).toBeNull()
    } finally {
      console.warn = original
    }
  })

  test('a stub presenter decides the answer; blank and rejecting ones count as cancel', async () => {
    setPromptDialogPresenter(async () => '  新名称  ')
    expect(await promptDialog(RENAME)).toBe('新名称')
    setPromptDialogPresenter(async () => '   ')
    expect(await promptDialog(RENAME)).toBeNull()
    setPromptDialogPresenter(async () => { throw new Error('boom') })
    expect(await promptDialog(RENAME)).toBeNull()
  })

  test('shows the Chinese title, prefills the current name, and is not titled with the dev server address', async () => {
    await mountProvider()
    const { answer } = await ask()
    const dialog = theDialog()
    expect(dialog.querySelector('[data-slot="dialog-title"]')?.textContent).toBe('重命名服务')
    expect(dialog.textContent).not.toContain('127.0.0.1')
    expect(input().value).toBe('DeepSeek')
    expect(input().getAttribute('aria-label')).toBe('服务名称')
    await act(async () => { fireEvent.click(button('取消')) })
    expect(await answer).toBeNull()
    await waitFor(() => expect(noDialog()).toBe(true))
  })

  test('Enter in the field confirms the trimmed new name', async () => {
    await mountProvider()
    const { answer } = await ask()
    await act(async () => { fireEvent.change(input(), { target: { value: '  深度求索  ' } }) })
    await act(async () => { fireEvent.keyDown(input(), { key: 'Enter' }) })
    expect(await answer).toBe('深度求索')
    await waitFor(() => expect(noDialog()).toBe(true))
  })

  test('Enter that commits an IME composition does not confirm', async () => {
    await mountProvider()
    const { answer } = await ask()
    let settled = false
    void answer.then(() => { settled = true })
    await act(async () => { fireEvent.change(input(), { target: { value: '深度' } }) })
    await act(async () => { fireEvent.keyDown(input(), { key: 'Enter', keyCode: 229 }) })
    await new Promise((resolve) => setTimeout(resolve, 10))
    expect(settled).toBe(false)
    expect(noDialog()).toBe(false)
    await act(async () => { fireEvent.keyDown(input(), { key: 'Enter' }) })
    expect(await answer).toBe('深度')
  })

  test('the 保存 button confirms too', async () => {
    await mountProvider()
    const { answer } = await ask()
    await act(async () => { fireEvent.change(input(), { target: { value: '公司网关' } }) })
    await act(async () => { fireEvent.click(button('保存')) })
    expect(await answer).toBe('公司网关')
  })

  test('Escape answers null', async () => {
    await mountProvider()
    const { answer } = await ask()
    await act(async () => { fireEvent.keyDown(input(), { key: 'Escape' }) })
    expect(await answer).toBeNull()
    await waitFor(() => expect(noDialog()).toBe(true))
  })

  test('an empty or whitespace-only name cannot be confirmed, by button or Enter', async () => {
    await mountProvider()
    const { answer } = await ask()
    let settled = false
    void answer.then(() => { settled = true })
    for (const blank of ['', '   ']) {
      await act(async () => { fireEvent.change(input(), { target: { value: blank } }) })
      expect(button('保存').disabled).toBe(true)
      await act(async () => { fireEvent.keyDown(input(), { key: 'Enter' }) })
      await act(async () => { fireEvent.submit(input().form!) })
      await act(async () => { fireEvent.click(button('保存')) })
      await new Promise((resolve) => setTimeout(resolve, 10))
      expect(settled).toBe(false)
      expect(noDialog()).toBe(false)
    }
    await act(async () => { fireEvent.change(input(), { target: { value: 'X' } }) })
    expect(button('保存').disabled).toBe(false)
    await act(async () => { fireEvent.submit(input().form!) })
    expect(await answer).toBe('X')
  })

  test('requests are shown one at a time, and unmounting cancels what is still open', async () => {
    let view!: ReturnType<typeof render>
    await act(async () => { view = render(React.createElement(PromptDialogProvider)) })
    let first!: Promise<string | null>
    let second!: Promise<string | null>
    await act(async () => {
      first = promptDialog({ ...RENAME, defaultValue: 'A' })
      second = promptDialog({ ...RENAME, defaultValue: 'B' })
    })
    await waitFor(() => expect(input().value).toBe('A'))
    await act(async () => { fireEvent.submit(input().form!) })
    expect(await first).toBe('A')
    await waitFor(() => expect(input().value).toBe('B'))
    await act(async () => { view.unmount() })
    expect(await second).toBeNull()
  })
})

describe('rename service uses the in-app prompt', () => {
  test('settings rename asks through promptDialog with the current name and syncs the list via save', async () => {
    const { readFileSync } = await import('node:fs')
    const { join } = await import('node:path')
    const src = join(import.meta.dir, '..', 'src')
    const settings = readFileSync(join(src, 'features/settings/settings-pages.tsx'), 'utf8')
    expect(settings).toContain("from '@/components/ui/prompt-dialog'")
    expect(settings).toMatch(/promptDialog\(\{ title: '重命名服务', label: '服务名称', defaultValue: provider\.name/)
    // A cancelled or unchanged name saves nothing; a new one goes through
    // resource.save, which refreshes the provider list.
    expect(settings).toMatch(/if \(!name \|\| name === provider\.name\) return\s*const saved = await resource\.save\(\{ id: provider\.id, name,/)
    const app = readFileSync(join(src, 'app', 'App.tsx'), 'utf8')
    expect(app).toContain('<PromptDialogProvider />')
  })
})
