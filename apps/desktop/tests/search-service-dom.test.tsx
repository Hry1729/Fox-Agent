import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const tauriCoreAsFound = { ...await import('@tauri-apps/api/core') }
type SearchView = { provider: string; searxngUrl: string; configuredKeys: string[] }
let current: SearchView = { provider: 'auto', searxngUrl: '', configuredKeys: [] }
let saveError: Error | null = null
const saves: Array<{ request: Record<string, unknown> }> = []

mock.module('@tauri-apps/api/core', () => ({
  ...tauriCoreAsFound,
  invoke: async (command: string, args: { request: Record<string, unknown> }) => {
    if (command === 'search_service_get') return { ...current, configuredKeys: [...current.configuredKeys] }
    if (command === 'search_service_save') {
      saves.push(args)
      if (saveError) throw saveError
      current = {
        provider: String(args.request.provider),
        searxngUrl: String(args.request.searxngUrl ?? ''),
        configuredKeys: args.request.apiKey ? [String(args.request.provider)] : current.configuredKeys,
      }
      return { ...current, configuredKeys: [...current.configuredKeys] }
    }
    throw new Error(`unexpected command: ${command}`)
  },
}))
afterAll(() => { mock.module('@tauri-apps/api/core', () => tauriCoreAsFound) })

const React = await import('react')
const { act, cleanup, fireEvent, render, waitFor } = await import('@testing-library/react')
const { SearchServiceSettings } = await import('../src/features/settings/search-service-page')
const { isSettingsSectionAnchor, settingsRouteRedirect, settingsNavigationView } = await import('../src/features/workspace/management-navigation')

function Harness() {
  const [open, setOpen] = React.useState(false)
  return React.createElement(SearchServiceSettings, { open, onOpenChange: setOpen })
}

const button = (name: string) => [...document.querySelectorAll('button')].find(item => item.textContent?.trim() === name) as HTMLButtonElement | undefined
const radio = (value: string) => document.querySelector(`input[type="radio"][value="${value}"]`) as HTMLInputElement | null
const sheet = () => document.querySelector('[data-slot="sheet-content"]') as HTMLElement | null
const input = (id: string) => document.getElementById(id) as HTMLInputElement | null

async function change(element: HTMLInputElement, value: string) {
  await act(async () => { fireEvent.change(element, { target: { value } }) })
}
async function choose(value: string) {
  await act(async () => { fireEvent.click(radio(value)!) })
}
async function open() {
  await act(async () => { fireEvent.click(document.querySelector('[aria-label="配置网页搜索"]')!) })
  await waitFor(() => {
    if (!sheet() || radio('auto')?.disabled) throw new Error('search sheet is not ready')
  })
}

afterEach(() => {
  cleanup()
  current = { provider: 'auto', searxngUrl: '', configuredKeys: [] }
  saveError = null
  saves.length = 0
})

describe('网页搜索 drawer', () => {
  test('keeps independent provider drafts, discards them on cancel, and saves key-free search', async () => {
    render(React.createElement(Harness))
    await open()
    expect(document.querySelectorAll('input[name="fox-search-provider"]')).toHaveLength(5)

    await choose('tavily')
    await change(input('fox-search-key-tavily')!, 'tavily-draft')
    await choose('brave')
    await change(input('fox-search-key-brave')!, 'brave-draft')
    await choose('tavily')
    expect(input('fox-search-key-tavily')!.value).toBe('tavily-draft')
    await choose('brave')
    expect(input('fox-search-key-brave')!.value).toBe('brave-draft')
    await choose('searxng')
    await change(input('fox-search-url')!, 'https://search.example.org')
    await choose('tavily')
    await choose('searxng')
    expect(input('fox-search-url')!.value).toBe('https://search.example.org')

    await act(async () => { fireEvent.click(button('取消')!) })
    await waitFor(() => { if (sheet()) throw new Error('sheet did not close') })
    await open()
    expect(radio('auto')!.checked).toBe(true)
    await choose('tavily')
    expect(input('fox-search-key-tavily')!.value).toBe('')
    await choose('searxng')
    expect(input('fox-search-url')!.value).toBe('')

    await choose('auto')
    await act(async () => { fireEvent.click(button('保存搜索')!) })
    await waitFor(() => { if (sheet()) throw new Error('sheet did not close after save') })
    expect(saves.at(-1)).toEqual({ request: { provider: 'auto', apiKey: null, searxngUrl: null } })
    await open()
    expect(radio('auto')!.checked).toBe(true)
  })

  test('keeps a save failure and its reason in the open drawer', async () => {
    saveError = new Error('测试保存失败')
    render(React.createElement(Harness))
    await open()
    await act(async () => { fireEvent.click(button('保存搜索')!) })
    await waitFor(() => {
      if (!sheet()?.querySelector('[role="alert"]')?.textContent?.includes('测试保存失败')) throw new Error('save error missing')
    })
    expect(sheet()).not.toBeNull()
    expect(button('取消')!.disabled).toBe(false)
  })

  test('recognizes search as a settings section anchor and preserves its legacy route', () => {
    expect(isSettingsSectionAnchor('search')).toBe(true)
    expect(settingsRouteRedirect('settings-search')).toEqual({ view: 'settings-extensions', section: 'search' })
    expect(settingsNavigationView('settings-search')).toBe('settings-extensions')
  })
})
