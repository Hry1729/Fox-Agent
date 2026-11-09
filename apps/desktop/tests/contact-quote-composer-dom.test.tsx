// C07：私聊输入框上方的引用草稿，以及「补充要求不支持引用」。
//
// 被测对象是 workbench 真正渲染的那个输入框（MemoizedComposer → FoxComposer），
// 挂到 happy-dom 上用真实键盘事件驱动；Host 边界用 mockIPC 替换，补充要求通道
// 按真实契约回答（run_steering_list / run_steering_enqueue）。

import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'

const calls: Array<{ command: string; request: Record<string, unknown> }> = []
const actualDesktopClient = await import('../src/features/conversations/api/desktop-client')
// 先快照：mock.module 会就地改写已导入的命名空间，直接引用它会把桩当成真身。
const realDesktopClient = { ...actualDesktopClient }

// `desktopRuntimeAvailable` 在模块加载时由 isTauri() 定死，所以补充要求通道
// 在这里按真实 Host 契约替换：Run 由会话决定，运行中可接收，enqueue 幂等。
mock.module('../src/features/conversations/api/desktop-client', () => ({
  ...realDesktopClient,
  desktopRuntimeAvailable: true,
  desktopClient: {
    ...realDesktopClient.desktopClient,
    listRunSteering: async (conversationId: string) => {
      calls.push({ command: 'run_steering_list', request: { conversationId } })
      return { runId: 'run', runState: 'running', acceptsSteering: true, messages: [] }
    },
    enqueueRunSteering: async (conversationId: string, content: string, options: { messageId?: string; lane?: string } = {}) => {
      calls.push({ command: 'run_steering_enqueue', request: { conversationId, content, ...options } })
      return { runId: 'run', seq: 1, messageId: options.messageId ?? 'server-1', status: 'received', lane: options.lane ?? 'current' }
    },
  },
}))

const React = await import('react')
const { cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const { TooltipProvider } = await import('../src/components/ui/tooltip')
const { MemoizedComposer } = await import('../src/features/chat/composer/fox-composer')
const { ComposerEntryContext } = await import('../src/features/chat/composer/composer-entry-context')
const { ContactsContext } = await import('../src/features/companions/context')
import type { QuotePreview } from '../src/features/companions/api'

const quoted: QuotePreview = { bubbleId: 'run:response:2', author: 'companion', summary: '华东机电的报价单没写税率。', deliveredAt: 1700000002000 }

function Harness({ initialQuote }: { initialQuote: QuotePreview | null }) {
  const [draft, setDraft] = React.useState('')
  const [quote, setQuote] = React.useState<QuotePreview | null>(initialQuote)
  const contacts = {
    privateChat: true, quoteDraft: quote, clearQuote: () => setQuote(null), showProfile: () => {},
    current: { id: 'contact', displayName: '资料伙伴', avatarRef: '📚', workspaceDir: '/tmp/contact' },
  }
  return <TooltipProvider>
    <ComposerEntryContext.Provider value={{ projects: [], onSelectProject: () => {}, onAddProject: () => {}, draftAgentBackend: 'fox', onSelectAgentBackend: () => {} }}>
      <ContactsContext.Provider value={contacts as never}>
        <MemoizedComposer resetKey={0} draft={draft} setDraft={setDraft} chatState="running" centered={false}
          newConversation={false} runtimeControlled steeringConversationId="chat" steeringActive />
      </ContactsContext.Provider>
    </ComposerEntryContext.Provider>
  </TooltipProvider>
}

afterEach(() => { cleanup(); calls.length = 0; window.localStorage.clear() })
// mock.module 是进程级的：本文件结束后把真实 Host 边界还回去，
// 免得后面的测试继承这里的补充要求桩。
afterAll(() => { mock.module('../src/features/conversations/api/desktop-client', () => realDesktopClient) })

describe('C07 引用草稿与补充要求', () => {
  test('输入框上方显示作者与摘要，取消后不再带入下一条消息', async () => {
    const view = render(<Harness initialQuote={quoted} />)
    const banner = view.container.querySelector('.fox-contact-quote-draft')
    expect(banner?.textContent).toContain('回复资料伙伴')
    expect(banner?.textContent).toContain('华东机电的报价单没写税率。')
    fireEvent.click(screen.getByLabelText('取消引用'))
    await waitFor(() => expect(view.container.querySelector('.fox-contact-quote-draft')).toBeNull())
  })

  test('运行中带引用时补充要求被拒绝且文字保留，取消引用后同一段文字才发得出去', async () => {
    const view = render(<Harness initialQuote={quoted} />)
    await waitFor(() => expect(calls.some(call => call.command === 'run_steering_list')).toBe(true))
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement
    fireEvent.change(textarea, { target: { value: '按这个税率重算' } })
    await waitFor(() => expect(textarea.value).toBe('按这个税率重算'))
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(view.container.querySelector('.fox-contact-quote-draft')).toBeTruthy())
    expect(calls.some(call => call.command === 'run_steering_enqueue')).toBe(false)
    expect(textarea.value).toBe('按这个税率重算')

    fireEvent.click(screen.getByLabelText('取消引用'))
    await waitFor(() => expect(view.container.querySelector('.fox-contact-quote-draft')).toBeNull())
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(calls.some(call => call.command === 'run_steering_enqueue')).toBe(true))
    expect(calls.find(call => call.command === 'run_steering_enqueue')?.request.content).toBe('按这个税率重算')
    await waitFor(() => expect(textarea.value).toBe(''))
  })
})