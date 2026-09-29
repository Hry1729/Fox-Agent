import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { createServer } from 'vite'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
globalThis.IS_REACT_ACT_ENVIRONMENT = true
const React = await import('react')
const { cleanup, fireEvent, render } = await import('@testing-library/react')
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const server = await createServer({
  root, configFile: false, appType: 'custom', cacheDir: path.join(root, 'dist', '.vite-test-cache'),
  optimizeDeps: { noDiscovery: true, include: [] }, server: { middlewareMode: true, hmr: false },
  resolve: { alias: { '@': path.join(root, 'src') } }, esbuild: { jsx: 'automatic' },
})
const { useProcessScroll } = await server.ssrLoadModule('/src/features/chat/use-process-scroll.ts')
after(async () => { cleanup(); await server.close() })

test('uncapped mode restores the observed group offset and body/content growth follows only at bottom', async () => {
  let height = 1000
  const observers = []
  const calls = []
  class TestResizeObserver {
    observed = []
    constructor(callback) { this.callback = callback; observers.push(this) }
    observe(node) { this.observed.push(node) }
    disconnect() { this.observed = [] }
    trigger() { this.callback([]) }
  }
  const oldObserver = globalThis.ResizeObserver
  const oldHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollHeight')
  const oldClient = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientHeight')
  const oldScrollTo = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollTo')
  globalThis.ResizeObserver = TestResizeObserver
  Object.defineProperty(HTMLElement.prototype, 'scrollHeight', { configurable: true, get() { return this.classList.contains('process-port') ? height : 0 } })
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get() { return this.classList.contains('process-port') ? 400 : 0 } })
  Object.defineProperty(HTMLElement.prototype, 'scrollTo', { configurable: true, value(options) {
    calls.push(options)
    if (options.behavior !== 'smooth') this.scrollTop = options.top
  } })

  function Harness({ capped, version }) {
    const bodyRef = React.useRef(null)
    const contentRef = React.useRef(null)
    const [open, setOpen] = React.useState(false)
    const { edges, events, initialize } = useProcessScroll(bodyRef, contentRef, open, capped, version)
    return React.createElement('div', null,
      React.createElement('button', { onClick: () => { initialize('bottom'); setOpen(true) } }, 'open'),
      React.createElement('div', { className: 'process-port', ...events,
        ref: node => { bodyRef.current = node; if (node && !capped) node.scrollTop = 0 } },
      React.createElement('div', { ref: contentRef }, 'content')),
      React.createElement('output', null, `${edges.up}/${edges.down}`))
  }

  try {
    const view = render(React.createElement(Harness, { capped: true, version: 1 }))
    fireEvent.click(view.getByText('open'))
    const body = view.container.querySelector('.process-port')
    assert.equal(body.scrollTop, 600)
    assert.ok(observers.some(observer => observer.observed.includes(body)
      && observer.observed.includes(body.firstChild)), 'body and content are both observed')
    body.scrollTop = 220
    fireEvent.scroll(body)
    view.rerender(React.createElement(Harness, { capped: false, version: 1 }))
    assert.equal(body.scrollTop, 0)
    view.rerender(React.createElement(Harness, { capped: true, version: 1 }))
    assert.equal(body.scrollTop, 220)

    height = 1400
    await React.act(async () => { observers.at(-1).trigger() })
    assert.equal(calls.filter(call => call.behavior === 'smooth').length, 0)
    body.scrollTop = 1000
    fireEvent.scroll(body)
    height = 1500
    await React.act(async () => { observers.at(-1).trigger() })
    assert.deepEqual(calls.filter(call => call.behavior === 'smooth'), [{ top: 1100, behavior: 'smooth' }])
    cleanup()
    assert.ok(observers.every(observer => observer.observed.length === 0), 'observers disconnect on unmount')
    const callCount = calls.length
    body.dispatchEvent(new Event('scrollend'))
    assert.equal(calls.length, callCount, 'scrollend listener is removed on unmount')
  } finally {
    cleanup()
    globalThis.ResizeObserver = oldObserver
    if (oldHeight) Object.defineProperty(HTMLElement.prototype, 'scrollHeight', oldHeight)
    else delete HTMLElement.prototype.scrollHeight
    if (oldClient) Object.defineProperty(HTMLElement.prototype, 'clientHeight', oldClient)
    else delete HTMLElement.prototype.clientHeight
    if (oldScrollTo) Object.defineProperty(HTMLElement.prototype, 'scrollTo', oldScrollTo)
    else delete HTMLElement.prototype.scrollTo
  }
})

