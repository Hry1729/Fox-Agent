// Real-DOM acceptance for the right sidebar's file flow: 「＋→文件」, one tab per
// opened file, and closing the last file again.
//
// SCOPE (honest): the rendered component is the real `RightPanel` — its tab bar,
// its ＋ page menu, its file tabs and their close buttons are the shipped ones.
// The *state* around it is a harness, because `Workbench` owns `openFile` /
// `closeFile` and can only be mounted with the whole Tauri IPC stack (project
// catalog, desktop client, notifications), which this process does not have. The
// harness applies the same two rules `Workbench` applies, quoted in the test, and
// the tool-row entry point that feeds `openFile` is covered in
// `runtime-process-row-dom.test.mjs` (clicking a row's path calls
// `onOpenFileInSidebar`).

import assert from 'node:assert/strict'
import { after, afterEach, test } from 'node:test'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { createServer } from 'vite'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
globalThis.IS_REACT_ACT_ENVIRONMENT = true
const React = await import('react')
const { cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const server = await createServer({
  root, configFile: false, appType: 'custom', cacheDir: path.join(root, 'dist', '.vite-panel-test-cache'),
  optimizeDeps: { noDiscovery: true, include: [] }, server: { middlewareMode: true, hmr: false, watch: { ignored: ['**/*'] } },
  resolve: { alias: { '@': path.join(root, 'src') } }, esbuild: { jsx: 'automatic' },
})
const { RightPanel } = await server.ssrLoadModule('/src/features/chat/workbench.tsx')
afterEach(cleanup)
after(async () => { await server.close() })

const panelProps = (overrides = {}) => ({
  mode: null, tabs: [], detail: null, activity: {}, width: 320, compact: false, maximized: false,
  fileTabs: [], activeFileTabId: null, activeChildAgent: null,
  onMode: () => {}, onCloseMode: () => {}, onOpenFile: () => {}, onActivateFile: () => {},
  onCloseFile: () => {}, onOpenChildAgent: () => {}, onBackChildAgent: () => {},
  onToggleMaximized: () => {}, onCollapse: () => {},
  ...overrides,
})

/** The tab identity `Workbench.createOpenFileTab` builds: a normalized path, its
 *  own last segment as the visible name, and the path itself as the tab id. */
const fileTab = (filePath) => ({
  id: filePath.replace(/\\/g, '/').replace(/^\.\//, ''),
  path: filePath.replace(/\\/g, '/').replace(/^\.\//, ''),
  name: filePath.replace(/\\/g, '/').split('/').at(-1),
  artifactId: null,
})

/** Stands in for `Workbench`: same open/close rules, real `RightPanel` rendering. */
function FilesHarness({ initialTabs = [], onCloseFile }) {
  const [pinned, setPinned] = React.useState([])
  const [mode, setMode] = React.useState(initialTabs.length ? 'files' : null)
  const [fileTabs, setFileTabs] = React.useState(initialTabs)
  const [activeFileTabId, setActiveFileTabId] = React.useState(initialTabs.at(-1)?.id ?? null)
  // Workbench.openFile: add the tab once, show the files page, activate the tab.
  const openFile = (filePath) => {
    const tab = fileTab(filePath)
    setFileTabs((current) => current.some((item) => item.id === tab.id) ? current : [...current, tab])
    setActiveFileTabId(tab.id)
    setMode('files')
  }
  // Workbench.closeFile: drop the tab, activate its neighbour, and leave the files
  // page only when the last file goes away and no ＋-pinned files page remains.
  const closeFile = (tabId) => {
    onCloseFile?.(tabId)
    const closingIndex = fileTabs.findIndex((item) => item.id === tabId)
    if (closingIndex < 0) return
    const nextTabs = fileTabs.filter((item) => item.id !== tabId)
    const nextActiveId = activeFileTabId === tabId
      ? nextTabs[Math.min(closingIndex, nextTabs.length - 1)]?.id ?? null
      : activeFileTabId
    setFileTabs(nextTabs)
    setActiveFileTabId(nextActiveId)
    if (mode === 'files' && activeFileTabId === tabId && !nextActiveId && !pinned.includes('files')) {
      setMode(pinned.at(-1) ?? null)
    }
  }
  return React.createElement(React.Fragment, null,
    React.createElement('button', { type: 'button', onClick: () => openFile('src/明细/a.ts') }, '打开文件'),
    React.createElement(RightPanel, panelProps({
      mode, tabs: pinned, fileTabs, activeFileTabId,
      onMode: (next) => { setPinned((current) => current.includes(next) ? current : [...current, next]); setMode(next) },
      onCloseMode: (next) => {
        const nextPinned = pinned.filter((item) => item !== next)
        setPinned(nextPinned)
        if (mode === next) setMode(nextPinned.at(-1) ?? null)
      },
      onCloseFile: closeFile,
    })))
}

test('the ＋ menu still offers 文件 and selecting it opens the files page', async () => {
  const modes = []
  const view = render(React.createElement(RightPanel, panelProps({ onMode: (mode) => modes.push(mode) })))
  const trigger = view.getByRole('button', { name: '打开侧边栏页面' })
  assert.ok(trigger)
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false })
  const item = await waitFor(() => screen.getByRole('menuitem', { name: '文件' }), { timeout: 3000 })
  fireEvent.click(item)
  assert.deepEqual(modes, ['files'])
})

test('opening a file adds its own tab, and closing the last one leaves no stale file view', async () => {
  const closed = []
  const view = render(React.createElement(FilesHarness, { onCloseFile: (tabId) => closed.push(tabId) }))
  assert.equal(view.container.querySelectorAll('.fox-context-tab.is-file-tab').length, 0)

  fireEvent.click(view.getByRole('button', { name: '打开文件' }))
  const tab = await waitFor(() => {
    const found = view.container.querySelector('.fox-context-tab.is-file-tab')
    assert.ok(found)
    return found
  })
  assert.equal(tab.querySelector('[role="tab"]').textContent, 'a.ts')
  assert.equal(tab.querySelector('[role="tab"]').getAttribute('title'), 'src/明细/a.ts')
  assert.equal(tab.querySelector('[role="tab"]').getAttribute('aria-selected'), 'true')

  // The file tab replaces the plain files page tab while any file is open.
  assert.equal([...view.container.querySelectorAll('.fox-context-tab [role="tab"]')]
    .filter((node) => node.textContent === '文件').length, 0)

  fireEvent.click(tab.querySelector('.fox-context-tab-close'))
  assert.deepEqual(closed, ['src/明细/a.ts'])
  await waitFor(() => assert.equal(view.container.querySelector('.fox-context-tab.is-file-tab'), null))
  // Nothing is left pinning the files page, so the panel falls back instead of
  // showing an empty file view, and the ＋ menu is still there to reopen it.
  assert.ok(view.getByRole('button', { name: '打开侧边栏页面' }))
  assert.equal(view.container.querySelector('.fox-file-panel'), null)
  assert.ok(view.container.querySelector('.fox-context-home'))
})
