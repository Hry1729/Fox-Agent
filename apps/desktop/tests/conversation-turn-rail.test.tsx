import { afterEach, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
const React = await import('react')
const { act, cleanup, fireEvent, render, waitFor } = await import('@testing-library/react')
const { computeActiveTurnId, ConversationTurnRail } = await import('../src/features/chat/timeline/fox-timeline')
afterEach(cleanup)

// ── 纯函数：SPEC item 8 的选中逻辑 ──────────────────────────────────────────

const spans = [
  { id: 'u1', top: -400, bottom: 100 },
  { id: 'u2', top: 120, bottom: 700 },
  { id: 'u3', top: 720, bottom: 1200 },
]
const viewport = { top: 0, bottom: 400 }

test('滚动到底时强制选中最后一轮，即使视口中线落在更早的轮次', () => {
  expect(computeActiveTurnId(spans, viewport, true)).toBe('u3')
})

test('非底部时选中覆盖视口中线的整轮范围', () => {
  // 中线 = 200，落在第二轮（120..700）内。
  expect(computeActiveTurnId(spans, viewport, false)).toBe('u2')
})

test('最后一轮的用户消息在屏幕外、助手回复可见时仍选中该轮', () => {
  const reading = [
    { id: 'u1', top: -900, bottom: -500 },
    { id: 'u2', top: -480, bottom: 300 }, // 用户锚点在视口上方，助手内容盖住中线 200
    { id: 'u3', top: 320, bottom: 900 },
  ]
  expect(computeActiveTurnId(reading, viewport, false)).toBe('u2')
})

test('中线停在两轮之间的空隙时取最近的一轮，距离相同取靠前的', () => {
  const gapped = [
    { id: 'u1', top: -100, bottom: 150 },
    { id: 'u2', top: 250, bottom: 600 },
  ]
  expect(computeActiveTurnId(gapped, viewport, false)).toBe('u1') // 中线 200：距 u1 50、距 u2 50 → 靠前
  const tilted = [
    { id: 'u1', top: -100, bottom: 120 },
    { id: 'u2', top: 250, bottom: 600 },
  ]
  expect(computeActiveTurnId(tilted, viewport, false)).toBe('u2') // 中线 200：距 u1 80、距 u2 50
})

test('没有轮次时返回 null', () => {
  expect(computeActiveTurnId([], viewport, true)).toBeNull()
  expect(computeActiveTurnId([], viewport, false)).toBeNull()
})

// ── 组件行为：真实挂载 rail + 伪造滚动视口 ─────────────────────────────────

type RectSpec = { top: number; bottom: number }

function message(id: string, role: 'user' | 'assistant', content: string) {
  return { id, role, content, runId: null, conversationId: 'c', kind: 'text', status: 'completed', ordinal: 1, createdAt: 1000, updatedAt: 1000 } as any
}

function RailHarness({ messages, rects, scroll }: {
  messages: Array<ReturnType<typeof message>>
  rects: Record<string, RectSpec>
  scroll: { scrollTop: number; scrollHeight: number; clientHeight: number }
}) {
  const setupViewport = React.useCallback((node: HTMLDivElement | null) => {
    if (!node) return
    Object.defineProperty(node, 'scrollTop', { get: () => scroll.scrollTop, set: () => {}, configurable: true })
    Object.defineProperty(node, 'scrollHeight', { get: () => scroll.scrollHeight, configurable: true })
    Object.defineProperty(node, 'clientHeight', { get: () => scroll.clientHeight, configurable: true })
    node.getBoundingClientRect = () => ({ top: 0, bottom: scroll.clientHeight, height: scroll.clientHeight, left: 0, right: 600, width: 600, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect
    node.scrollTo = () => {}
  }, [scroll.scrollTop, scroll.scrollHeight, scroll.clientHeight])
  const setupAnchor = (id: string) => (node: HTMLDivElement | null) => {
    if (!node) return
    node.getBoundingClientRect = () => {
      const rect = rects[id] ?? { top: 0, bottom: 0 }
      return { top: rect.top, bottom: rect.bottom, height: rect.bottom - rect.top, left: 0, right: 600, width: 600, x: 0, y: rect.top, toJSON: () => ({}) } as DOMRect
    }
  }
  return <div className="fox-conversation">
    <div className="fox-conversation-scroll" ref={setupViewport} style={{ overflowY: 'auto' }}>
      {messages.map((entry) => <div key={entry.id} className="fox-turn-anchor" data-turn-message-id={entry.id} ref={setupAnchor(entry.id)}>{entry.content}</div>)}
    </div>
    <ConversationTurnRail messages={messages} artifacts={[]} />
  </div>
}

function activeTick(view: { container: HTMLElement }) {
  const active = view.container.querySelectorAll('.fox-conversation-marks button.is-active')
  return active.length === 1 ? active[0].getAttribute('aria-label') : `count:${active.length}`
}

const baseMessages = [
  message('u1', 'user', '第一问'),
  message('a1', 'assistant', '第一答'),
  message('u2', 'user', '第二问'),
  message('a2', 'assistant', '第二答'),
]

test('底部：两轮都在视口内时选中最后一轮', async () => {
  const view = render(<RailHarness messages={baseMessages} rects={{ u1: { top: 40, bottom: 80 }, a1: { top: 90, bottom: 160 }, u2: { top: 200, bottom: 240 }, a2: { top: 250, bottom: 360 } }} scroll={{ scrollTop: 600, scrollHeight: 1000, clientHeight: 400 }} />)
  await waitFor(() => expect(activeTick(view)).toBe('第 2 轮：第二问'))
})

test('中线阅读：最后一轮用户消息在屏幕外但助手可见时选中最后一轮', async () => {
  const view = render(<RailHarness messages={baseMessages} rects={{ u1: { top: -600, bottom: -560 }, a1: { top: -550, bottom: -100 }, u2: { top: -80, bottom: -40 }, a2: { top: 60, bottom: 360 } }} scroll={{ scrollTop: 300, scrollHeight: 1000, clientHeight: 400 }} />)
  await waitFor(() => expect(activeTick(view)).toBe('第 2 轮：第二问'))
})

test('中部阅读：中线落在第一轮范围内时选中第一轮', async () => {
  const view = render(<RailHarness messages={baseMessages} rects={{ u1: { top: -100, bottom: -60 }, a1: { top: 0, bottom: 300 }, u2: { top: 340, bottom: 380 }, a2: { top: 390, bottom: 700 } }} scroll={{ scrollTop: 300, scrollHeight: 1000, clientHeight: 400 }} />)
  await waitFor(() => expect(activeTick(view)).toBe('第 1 轮：第一问'))
})

test('流式追加：新一轮到达并重排后选中项跟随滚动位置更新', async () => {
  const rects: Record<string, RectSpec> = { u1: { top: -100, bottom: -60 }, a1: { top: 0, bottom: 300 }, u2: { top: 340, bottom: 380 }, a2: { top: 390, bottom: 700 } }
  const scroll = { scrollTop: 300, scrollHeight: 1000, clientHeight: 400 }
  const view = render(<RailHarness messages={baseMessages} rects={rects} scroll={scroll} />)
  await waitFor(() => expect(activeTick(view)).toBe('第 1 轮：第一问'))
  // 第三轮流式进入，且用户回到底部：内容长高、scrollTop 跟到底。
  rects.u3 = { top: 740, bottom: 780 }
  rects.a3 = { top: 790, bottom: 820 }
  scroll.scrollTop = 1000
  scroll.scrollHeight = 1400
  await act(async () => view.rerender(<RailHarness messages={[...baseMessages, message('u3', 'user', '第三问'), message('a3', 'assistant', '正在回复…')]} rects={rects} scroll={scroll} />))
  await waitFor(() => expect(activeTick(view)).toBe('第 3 轮：第三问'))
  // 向上滚回第二轮（scrollTop 300 → 500，所有锚点相对视口上移 200）：
  // scroll 事件驱动重新量测，中线 200 落回第二轮的助手范围。
  rects.u1 = { top: -300, bottom: -260 }
  rects.a1 = { top: -200, bottom: 100 }
  rects.u2 = { top: 140, bottom: 180 }
  rects.a2 = { top: 190, bottom: 500 }
  rects.u3 = { top: 540, bottom: 580 }
  rects.a3 = { top: 590, bottom: 620 }
  scroll.scrollTop = 500
  await act(async () => {
    view.container.querySelector('.fox-conversation-scroll')!.dispatchEvent(new Event('scroll'))
  })
  await waitFor(() => expect(activeTick(view)).toBe('第 2 轮：第二问'))
})

test('点击刻度滚动到该轮并立即选中', async () => {
  const view = render(<RailHarness messages={baseMessages} rects={{ u1: { top: -100, bottom: -60 }, a1: { top: 0, bottom: 300 }, u2: { top: 340, bottom: 380 }, a2: { top: 390, bottom: 700 } }} scroll={{ scrollTop: 300, scrollHeight: 1000, clientHeight: 400 }} />)
  await waitFor(() => expect(view.container.querySelectorAll('.fox-conversation-marks button')).toHaveLength(2))
  fireEvent.click(view.container.querySelectorAll('.fox-conversation-marks button')[1])
  expect(activeTick(view)).toBe('第 2 轮：第二问')
})

test('选中刻度保持短横线（不再加长加亮为强调色），仅悬停/聚焦变长', () => {
  // 视觉规格由 CSS 承载（workbench.css .fox-conversation-marks）；这里锁定 DOM 结构：
  // 每个刻度是一个真实 button，内含单个 span，选中态只加 is-active 类。
  const view = render(<RailHarness messages={baseMessages} rects={{ u1: { top: 0, bottom: 40 }, a1: { top: 50, bottom: 120 }, u2: { top: 160, bottom: 200 }, a2: { top: 210, bottom: 300 } }} scroll={{ scrollTop: 600, scrollHeight: 1000, clientHeight: 400 }} />)
  const buttons = view.container.querySelectorAll('.fox-conversation-marks button')
  expect(buttons).toHaveLength(2)
  for (const button of buttons) {
    expect(button.tagName).toBe('BUTTON')
    expect(button.children).toHaveLength(1)
    expect(button.children[0].tagName).toBe('SPAN')
  }
})
