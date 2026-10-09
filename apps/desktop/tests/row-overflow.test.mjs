import assert from 'node:assert/strict'
import { after, afterEach, test } from 'node:test'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { createServer } from 'vite'

/**
 * The trailing fade is a layout decision: it may only appear when the text really
 * does not fit the line. happy-dom has no layout and reports 0 for every box, so
 * this file supplies the measurements itself (a simulated one-line layout plus a
 * controllable ResizeObserver) and lets the real hook, the real decision helper and
 * the real rows produce the state. Nothing here asserts a CSS string as proof that
 * the fade appears.
 */
if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
globalThis.IS_REACT_ACT_ENVIRONMENT = true
const React = await import('react')
const { cleanup, fireEvent, render, waitFor } = await import('@testing-library/react')
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const server = await createServer({
  root, configFile: false, appType: 'custom', cacheDir: path.join(root, 'dist', '.vite-overflow-test-cache'),
  optimizeDeps: { noDiscovery: true, include: [] }, server: { middlewareMode: true, hmr: false, watch: { ignored: ['**/*'] } },
  resolve: { alias: { '@': path.join(root, 'src') } }, esbuild: { jsx: 'automatic' },
})
const { isOverflowing, useOverflowFade, OVERFLOW_CLASS } = await server.ssrLoadModule('/src/features/chat/row-overflow.ts')
const { RuntimeTimeline } = await server.ssrLoadModule('/src/features/chat/workbench.tsx')
const { persistProcessDisplayMode } = await server.ssrLoadModule('/src/features/chat/process-display-mode.ts')

const teardowns = []
afterEach(() => {
  cleanup()
  persistProcessDisplayMode('standard')
  while (teardowns.length) teardowns.pop()()
})
after(async () => { await server.close() })

/** One simulated line: `scrollWidth` is the intrinsic width of the element's own
 *  text (CJK counts wider than Latin, so the two scripts are not interchangeable),
 *  and `clientWidth` is the line the row currently has. Both numbers are the test's
 *  to move, which is exactly how a narrower sidebar, a wider window or a different
 *  font would move them. */
function installLineLayout({ lineWidth = 240, latin = 7, cjk = 13 } = {}) {
  const state = { lineWidth, latin, cjk }
  const intrinsicWidth = (text) => [...text]
    .reduce((total, char) => total + (char.codePointAt(0) > 0x2e80 ? state.cjk : state.latin), 0)
  const clientWidth = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth')
  const scrollWidth = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollWidth')
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get() { return state.lineWidth } })
  Object.defineProperty(Element.prototype, 'scrollWidth', { configurable: true, get() { return intrinsicWidth(this.textContent ?? '') } })
  teardowns.push(() => {
    if (clientWidth) Object.defineProperty(HTMLElement.prototype, 'clientWidth', clientWidth)
    if (scrollWidth) Object.defineProperty(Element.prototype, 'scrollWidth', scrollWidth)
  })
  return { state }
}

/** A controllable stand-in for the platform observer: the hook must ask *it* to
 *  re-read, so a test can deliver exactly the notification a real resize would. */
function installObserverProbe() {
  const instances = []
  class ProbeResizeObserver {
    constructor(callback) { this.callback = callback; this.targets = new Set(); this.disconnected = false; instances.push(this) }
    observe(target) { this.targets.add(target) }
    unobserve(target) { this.targets.delete(target) }
    disconnect() { this.targets.clear(); this.disconnected = true }
  }
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'ResizeObserver')
  Object.defineProperty(globalThis, 'ResizeObserver', { configurable: true, writable: true, value: ProbeResizeObserver })
  teardowns.push(() => {
    if (previous) Object.defineProperty(globalThis, 'ResizeObserver', previous)
    else delete globalThis.ResizeObserver
  })
  return {
    instances,
    observersFor(target) { return instances.filter(observer => observer.targets.has(target)) },
    /** One "this element may have changed size" notification, as a resized window,
     *  a narrowed sidebar, a changed font or a re-shown row would deliver it. The
     *  entries carry the measured box, because the app mounts other observers too. */
    resize() {
      return React.act(async () => {
        for (const observer of instances) {
          if (observer.disconnected || !observer.targets.size) continue
          const entries = [...observer.targets].map(target => ({
            target, contentRect: { width: target.clientWidth, height: target.clientHeight },
          }))
          observer.callback(entries, observer)
        }
      })
    },
  }
}

function OverflowProbe({ text, enabled = true }) {
  const { ref, overflowing } = useOverflowFade(text, enabled)
  return React.createElement('span', { ref, className: overflowing ? OVERFLOW_CLASS : '' }, text)
}

const user = { id: 'user', conversationId: 'conversation', runId: 'run', role: 'user', kind: 'text',
  content: '请处理', status: 'completed', ordinal: 1, createdAt: 1, updatedAt: 1 }
const assistant = { id: 'assistant-run', conversationId: 'conversation', runId: 'run',
  role: 'assistant', kind: 'text', content: '示例答复', status: 'streaming', ordinal: 2, createdAt: 2, updatedAt: 2 }
const event = (seq, eventType, data = {}) => ({ runId: 'run', seq, eventType, event: { type: eventType, ...data }, createdAt: seq })
const timeline = (reasoning, overrides = {}) => React.createElement(RuntimeTimeline, {
  messages: [user, assistant],
  attachments: [], artifacts: [], events: reasoning ? [event(1, 'reasoning.delta', { delta: reasoning })] : [],
  runtimeRunning: true, activeRunId: 'run', state: 'streaming', streamingText: '', onRetry: () => {}, onRerun: async () => false,
  ...overrides,
})
/** A live thought belongs to the still-streaming tail of the turn: the answer has
 *  not started, so its process group is the live one. */
const liveThought = (reasoning, overrides = {}) => timeline(reasoning, {
  messages: [user, { ...assistant, content: '' }], ...overrides,
})

/** Open the turn disclosure (when the turn owns one) and the stage disclosure, and
 *  return the requested row. Grouping helpers arrive asynchronously and can remount
 *  the process, so a disclosure click is repeated until the row really exists. */
async function openFirstRow(view, selector = '.fox-runtime-step-row') {
  await waitFor(() => assert.ok(view.container.querySelector('.fox-chain-of-thought-header')), { timeout: 5000 })
  const expand = view.queryByRole('button', { name: /展开过程/ })
  if (expand) fireEvent.click(expand)
  await waitFor(() => {
    const header = view.container.querySelector('.fox-chain-of-thought-header')
    if (header?.getAttribute('aria-expanded') !== 'true') fireEvent.click(header)
    assert.ok(view.container.querySelector(selector))
  }, { timeout: 5000 })
  return view.container.querySelector(selector)
}

const overflows = (node) => node.classList.contains(OVERFLOW_CLASS)

test('overflow is decided by the measured line, never by the length of the text', () => {
  // Real layout: the box is narrower than what is inside it.
  assert.equal(isOverflowing({ scrollWidth: 120, clientWidth: 80 }), true)
  // A 1px tolerance for sub-pixel rounding: a fraction wider is not truncated.
  assert.equal(isOverflowing({ scrollWidth: 80.4, clientWidth: 80 }), false)
  assert.equal(isOverflowing({ scrollWidth: 81, clientWidth: 80 }), false)
  assert.equal(isOverflowing({ scrollWidth: 80, clientWidth: 80 }), false)
  assert.equal(isOverflowing({ scrollWidth: 81.5, clientWidth: 80 }), true)
  assert.equal(isOverflowing({ scrollWidth: 82, clientWidth: 80 }), true)
  // An unmeasurable box — a hidden row, or a document without layout — is never
  // treated as truncated, so a short title can never be masked by accident.
  assert.equal(isOverflowing({ scrollWidth: 0, clientWidth: 0 }), false)
  assert.equal(isOverflowing({ scrollWidth: 900, clientWidth: 0 }), false)
  assert.equal(isOverflowing({ scrollWidth: 900, clientWidth: -1 }), false)
})

test('the class follows the measurement: content growth, a resize, a font change, and a disabled element', async () => {
  const layout = installLineLayout({ lineWidth: 200 })
  const observer = installObserverProbe()
  const view = render(React.createElement(OverflowProbe, { text: '短标题' }))
  const node = view.container.querySelector('span')
  assert.equal(overflows(node), false)

  // The content grows while streaming: the clip box itself does not resize, so only
  // the content-triggered re-read can see it.
  await React.act(async () => { view.rerender(React.createElement(OverflowProbe, { text: '短标题'.repeat(12) })) })
  assert.equal(overflows(node), true)
  assert.equal(observer.observersFor(node).length, 1)

  // Narrowing the line (sidebar, window, zoom) is what the observer is for.
  layout.state.lineWidth = 20
  await observer.resize()
  assert.equal(overflows(node), true)

  // Widening it takes the fade away again.
  layout.state.lineWidth = 4000
  await observer.resize()
  assert.equal(overflows(node), false)

  // A wider font makes the same string overflow without any resize of the box.
  layout.state.lineWidth = 200
  layout.state.latin = 60
  layout.state.cjk = 60
  await observer.resize()
  assert.equal(overflows(node), true)

  // A disabled measurement never reports overflow, however long the text is.
  layout.state.lineWidth = 20
  await React.act(async () => { view.rerender(React.createElement(OverflowProbe, { text: '短标题'.repeat(12), enabled: false })) })
  assert.equal(overflows(node), false)

  // One observer per element, and it is disconnected when the element goes away.
  view.unmount()
  assert.equal(observer.instances.some(item => item.disconnected), true)
  assert.equal(observer.observersFor(node).some(item => !item.disconnected), false)
})

test('a short thinking preview is not masked, while CJK, English and a long path all fade', async () => {
  installLineLayout({ lineWidth: 400 })
  const short = render(liveThought('先分析'))
  const shortLabel = (await openFirstRow(short, '.fox-runtime-step-row.is-reasoning')).querySelector('.fox-run-status-label')
  assert.equal(shortLabel.textContent, '先分析')
  assert.equal(overflows(shortLabel), false)
  cleanup()

  // Each of these is decided by the width it really occupies — not by a character
  // count, and not by whether the script is CJK or Latin.
  const cases = [
    '这是一条很长的中文思考摘要，长到一行无论如何都放不下，需要在行末渐隐',
    'This is a deliberately long reasoning summary that cannot fit on one line at all',
    'src/功能模块/非常长的英文目录名称/组件/渲染器/RuntimeProcessRowImplementation.tsx',
  ]
  for (const text of cases) {
    const view = render(liveThought(text))
    const row = await openFirstRow(view, '.fox-runtime-step-row.is-reasoning')
    const label = row.querySelector('.fox-run-status-label')
    assert.equal(label.textContent, text, 'the row still holds the whole content')
    assert.equal(overflows(label), true, text.slice(0, 12))
    assert.equal(overflows(row.querySelector('.fox-run-status-prefix')), false)
    cleanup()
  }
})

test('a narrowing sidebar adds the fade and a widening one removes it', async () => {
  const layout = installLineLayout({ lineWidth: 4000 })
  const observer = installObserverProbe()
  const view = render(liveThought('正在核对输出 第二行'))
  const row = await openFirstRow(view, '.fox-runtime-step-row.is-reasoning')
  const label = row.querySelector('.fox-run-status-label')
  assert.equal(overflows(label), false)

  layout.state.lineWidth = 60
  await observer.resize()
  assert.equal(overflows(label), true)

  layout.state.lineWidth = 4000
  await observer.resize()
  assert.equal(overflows(label), false)
})

test('a streaming preview grows into the fade without any resize', async () => {
  installLineLayout({ lineWidth: 200 })
  const view = render(liveThought('先分析'))
  await openFirstRow(view, '.fox-runtime-step-row.is-reasoning')
  const label = () => view.container.querySelector('.fox-runtime-step-row.is-reasoning .fox-run-status-label')
  assert.equal(overflows(label()), false)

  // The thought streams in. The clip box does not change size while the text grows,
  // so only the content-driven re-read can notice that the line no longer fits.
  await React.act(async () => { view.rerender(liveThought('先分析并逐字写入的更长的思考内容到这里已经放不下这一整行了')) })
  assert.equal(label().textContent, '先分析并逐字写入的更长的思考内容到这里已经放不下这一整行了')
  assert.equal(overflows(label()), true)

  // A shorter, later thought goes back to a full paint.
  await React.act(async () => { view.rerender(liveThought('先分析')) })
  assert.equal(label().textContent, '先分析')
  assert.equal(overflows(label()), false)
})

test('a slotted row fades only its source/summary tail, never the verb or the path', async () => {
  const longPath = 'src/功能模块/非常长的英文目录名称/组件/渲染器/RuntimeProcessRowImplementation.tsx'
  const longSource = '销售明细汇总工作表 Mixed Sheet Name With A Long Tail'
  const toolEvents = (source) => [
    event(1, 'tool.started', { toolCallId: 'read', tool: 'read', input: { path: longPath } }),
    event(2, 'tool.completed', { toolCallId: 'read', tool: 'read', result: { sheets: [{ name: source }] } }),
  ]
  const rowOf = (source) => timeline('', { events: toolEvents(source), onOpenFileInSidebar: () => {} })
  const layout = installLineLayout({ lineWidth: 4000 })
  const observer = installObserverProbe()

  // A short supplementary source shows in full, with no fade at all.
  const short = render(rowOf('明细'))
  const shortRow = await openFirstRow(short)
  assert.equal(shortRow.querySelector('.fox-run-step-tail').textContent, '明细')
  assert.equal(overflows(shortRow.querySelector('.fox-run-step-tail')), false)
  cleanup()

  // A long one still fits this line, so it is not faded either.
  const view = render(rowOf(longSource))
  const row = await openFirstRow(view)
  const tail = () => row.querySelector('.fox-run-step-tail')
  assert.equal(tail().textContent, longSource)
  assert.equal(overflows(tail()), false)
  assert.equal(observer.observersFor(tail()).length, 1)

  // Then the panel narrows: only the supplementary region fades.
  layout.state.lineWidth = 120
  await observer.resize()
  assert.equal(overflows(tail()), true)
  assert.equal(overflows(row.querySelector('.fox-run-step-action')), false)
  assert.equal(overflows(row.querySelector('.fox-run-step-path-slot')), false)
  assert.equal(row.querySelector('.fox-run-step-action').textContent, '读取文件')
  assert.equal(row.querySelector('.fox-runtime-step-path')?.textContent, longPath)
  assert.equal(row.querySelector('.fox-run-step-source').textContent, longSource)
})

test('compact mode decides the preview by the same measurement', async () => {
  await React.act(async () => persistProcessDisplayMode('compact'))
  const layout = installLineLayout({ lineWidth: 4000 })
  const observer = installObserverProbe()
  const settled = { runtimeRunning: false, activeRunId: undefined, state: 'idle' }
  const view = render(timeline('**结算首行**\n后续正文', settled))
  const row = await openFirstRow(view, '.fox-runtime-step-row.is-reasoning')
  const label = () => row.querySelector('.fox-run-status-label')
  assert.equal(label().textContent, '结算首行 后续正文')
  assert.equal(overflows(label()), false)

  layout.state.lineWidth = 30
  await observer.resize()
  assert.equal(overflows(label()), true)

  layout.state.lineWidth = 4000
  await observer.resize()
  assert.equal(overflows(label()), false)
})

test('opening a row clears the fade and collapsing it measures the line again', async () => {
  installLineLayout({ lineWidth: 40 })
  const view = render(liveThought('先分析并核对输出'))
  const row = await openFirstRow(view, '.fox-runtime-step-row.is-reasoning')
  const label = () => row.querySelector('.fox-run-status-label')
  assert.equal(overflows(label()), true)
  fireEvent.click(row.querySelector('.fox-runtime-step-trigger'))
  assert.equal(row.getAttribute('data-state'), 'open')
  // The open row hides its one-line summary, so there is no clipped line left to fade.
  assert.equal(label().textContent, '')
  assert.equal(overflows(label()), false)
  fireEvent.click(row.querySelector('.fox-runtime-step-trigger'))
  assert.equal(row.getAttribute('data-state'), 'closed')
  assert.equal(overflows(label()), true)
})
