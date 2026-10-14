// Real chat tree and Base/Motion components; no accordion or state mocks.
import assert from 'node:assert/strict'
import { after, afterEach, test } from 'node:test'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { createServer } from 'vite'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
globalThis.IS_REACT_ACT_ENVIRONMENT = true
// Happy DOM eagerly creates `finished`, unlike the browser's lazy getter.
// Motion cancels WAAPI animations without consuming that otherwise-unused
// promise. Observe only the expected cancellation; preserve all other errors.
const nativeAnimate = Element.prototype.animate
Element.prototype.animate = function (...args) {
  const animation = nativeAnimate.apply(this, args)
  animation.finished.catch(error => { if (error.name !== 'AbortError') throw error })
  return animation
}
const React = await import('react')
const { cleanup, fireEvent, render, waitFor } = await import('@testing-library/react')
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const server = await createServer({
  root, configFile: false, appType: 'custom', cacheDir: path.join(root, 'dist', '.vite-tree-test-cache'),
  optimizeDeps: { noDiscovery: true, include: [] }, server: { middlewareMode: true, hmr: false, watch: { ignored: ['**/*'] } },
  resolve: { alias: { '@': path.join(root, 'src') } }, esbuild: { jsx: 'automatic' },
})
const { ProjectFileTree } = await server.ssrLoadModule('/src/features/chat/right-panel/fox-right-panel.tsx')
afterEach(cleanup)
after(async () => { await server.close(); Element.prototype.animate = nativeAnimate })

const directory = (filePath, parent = '') => ({ path: filePath, name: filePath.split('/').at(-1), parent, isDirectory: true, size: 0 })
const file = (filePath, parent) => ({ ...directory(filePath, parent), isDirectory: false })
const entries = [directory('src'), directory('src/deep', 'src'), file('src/deep/a.ts', 'src/deep'), directory('docs'), directory('docs/guide', 'docs'), file('docs/guide/readme.md', 'docs/guide')]
const props = { entries, workspaceName: 'workspace', selectedFile: '', onSelect: () => {} }
function expanded(view, name, expected = true) {
  assert.equal(view.getByRole('button', { name, exact: true }).getAttribute('aria-expanded'), String(expected))
}

test('chat file tree keeps sibling/ancestor expansion, and file clicks still select the exact path', async () => {
  const selected = []
  const view = render(React.createElement(ProjectFileTree, { ...props, onSelect: value => selected.push(value) }))
  for (const name of ['src', 'docs', 'deep', 'guide']) fireEvent.click(view.getByRole('button', { name, exact: true }))
  for (const name of ['workspace', 'src', 'docs', 'deep', 'guide']) expanded(view, name)
  fireEvent.click(view.getByRole('button', { name: '打开 a.ts' }))
  fireEvent.click(view.getByRole('button', { name: '打开 readme.md' }))
  assert.deepEqual(selected, ['src/deep/a.ts', 'docs/guide/readme.md'])
  fireEvent.click(view.getByRole('button', { name: 'deep', exact: true }))
  expanded(view, 'deep', false)
  for (const name of ['workspace', 'src', 'docs', 'guide']) expanded(view, name)
  fireEvent.click(view.getByRole('button', { name: 'src', exact: true }))
  expanded(view, 'src', false)
  expanded(view, 'docs')
  expanded(view, 'guide')
})

test('external selection reveals normalized ancestors without losing manual expansion or overriding manual collapse', async () => {
  const view = render(React.createElement(ProjectFileTree, props))
  fireEvent.click(view.getByRole('button', { name: 'docs', exact: true }))
  fireEvent.click(view.getByRole('button', { name: 'guide', exact: true }))
  const selectedProps = { ...props, selectedFile: '.\\SRC\\deep\\a.ts' }
  view.rerender(React.createElement(ProjectFileTree, selectedProps))
  await waitFor(() => {
    for (const name of ['workspace', 'src', 'deep', 'docs', 'guide']) expanded(view, name)
    assert.equal(view.getByRole('button', { name: '打开 a.ts' }).getAttribute('aria-current'), 'true')
  })
  fireEvent.click(view.getByRole('button', { name: 'src', exact: true }))
  expanded(view, 'src', false)
  view.rerender(React.createElement(ProjectFileTree, selectedProps))
  expanded(view, 'src', false)
  expanded(view, 'guide')
  view.rerender(React.createElement(ProjectFileTree, { ...props, selectedFile: 'docs/guide/readme.md' }))
  await waitFor(() => assert.equal(view.getByRole('button', { name: '打开 readme.md' }).getAttribute('aria-current'), 'true'))
  expanded(view, 'src', false)
  expanded(view, 'docs')
  expanded(view, 'guide')
})

test('selection arriving before the async file catalog is revealed after entries load', async () => {
  const view = render(React.createElement(ProjectFileTree, { ...props, entries: [], selectedFile: 'src/deep/a.ts' }))
  view.rerender(React.createElement(ProjectFileTree, { ...props, selectedFile: 'src/deep/a.ts' }))
  await waitFor(() => {
    for (const name of ['workspace', 'src', 'deep']) expanded(view, name)
    assert.equal(view.getByRole('button', { name: '打开 a.ts' }).getAttribute('aria-current'), 'true')
  })
  expanded(view, 'docs', false)
})
