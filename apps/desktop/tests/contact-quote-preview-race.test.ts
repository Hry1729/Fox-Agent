// C07：异步引用预览的会话/请求失效检查。
//
// 被测对象是 workbench 真实的 quoteContactBubble 与切换会话时的重置 effect
// （与 local-knowledge-document-draft.test.ts 同一套源码抽取方式）：
// 切换联系人后旧预览不得填入新会话的草稿，连续点引用只有最新一次生效。

import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import type { QuotePreview } from '../src/features/companions/api'

const workbench = readFileSync(new URL('../src/features/chat/workbench.tsx', import.meta.url), 'utf8')
const quoteSource = workbench.slice(workbench.indexOf('  const getSelectedConversationId ='), workbench.indexOf('  const clearContactQuote ='))
const resetSource = workbench.slice(workbench.indexOf('  useEffect(() => {\n    setContactLocateRequest(null)'), workbench.indexOf('  const contactsContext ='))

const preview = (bubbleId: string): QuotePreview => ({ bubbleId, author: 'companion', summary: `摘要 ${bubbleId}`, deliveredAt: 1700000002000 })

function harness() {
  const drafts: Array<QuotePreview | null> = []
  const request = { current: { conversationId: null as string | null, version: 0 } }
  let contact = { id: 'contact-a', conversationId: 'chat-a' }
  let desktopConversation = { selectedConversationId: 'chat-a' }
  let reset: () => void = () => {}
  const resolvers = new Map<string, (value: QuotePreview) => void>()
  const previewCalls: Array<{ companionId: string; bubbleId: string }> = []
  const errors: unknown[] = []
  type Callback = (...args: unknown[]) => unknown
  const callbacks: Array<{ current: Callback; stable: Callback }> = []
  let callbackIndex = 0
  const env = {
    useStableCallback: (callback: Callback) => {
      const index = callbackIndex++
      const existing = callbacks[index]
      if (existing) {
        existing.current = callback
        return existing.stable
      }
      const entry: { current: Callback; stable: Callback } = {
        current: callback,
        stable: (...args) => entry.current(...args),
      }
      callbacks.push(entry)
      return entry.stable
    },
    useEffect: (effect: () => void) => { reset = effect },
    contacts: {
      quotePreview: (companionId: string, bubbleId: string) => {
        previewCalls.push({ companionId, bubbleId })
        return new Promise<QuotePreview>((resolve) => { resolvers.set(bubbleId, resolve) })
      },
    },
    contactQuoteRequestRef: request,
    setContactQuote: (value: QuotePreview | null) => { drafts.push(value) },
    setContactLocateRequest: () => {},
    setContactMarkUpdates: () => {},
    showContactError: (error: unknown) => { errors.push(error) },
  }
  const compiled = new Bun.Transpiler({ loader: 'tsx' }).transformSync(`${quoteSource}\n${resetSource}`)
  const renderSource = new Function(...Object.keys(env), 'contact', 'desktopConversation', `${compiled}; return quoteContactBubble`)
  const render = () => {
    callbackIndex = 0
    return renderSource(...Object.values(env), contact, desktopConversation) as (bubbleId: string) => Promise<void>
  }
  const quoteBubble = render()
  reset()
  drafts.length = 0
  return {
    quoteBubble,
    drafts,
    errors,
    previewCalls,
    request,
    resolve: (bubbleId: string) => resolvers.get(bubbleId)!(preview(bubbleId)),
    selectConversation: (conversationId: string, runReset = true) => {
      desktopConversation = { selectedConversationId: conversationId }
      render()
      if (runReset) reset()
    },
    completeDetailLoad: (conversationId: string) => {
      contact = { id: `contact-${conversationId}`, conversationId }
      render()
    },
  }
}

describe('C07 引用预览的失效检查', () => {
  test('当前会话的预览正常填入草稿', async () => {
    const h = harness()
    const pending = h.quoteBubble('run:response:2')
    h.resolve('run:response:2')
    await pending
    expect(h.drafts).toEqual([preview('run:response:2')])
    expect(h.errors).toEqual([])
  })

  test('切换联系人后，旧会话的预览不再填入草稿', async () => {
    const h = harness()
    const pending = h.quoteBubble('run:response:2')
    h.selectConversation('chat-b')
    h.completeDetailLoad('chat-b')
    expect(h.drafts).toEqual([null])
    h.resolve('run:response:2')
    await pending
    expect(h.drafts).toEqual([null])
  })

  test('已选中 B 但详情仍为 A 时，残留 A 气泡不能发起预览或改写请求', async () => {
    const h = harness()
    h.selectConversation('chat-b')
    const resetRequest = { ...h.request.current }
    expect(resetRequest.conversationId).toBe('chat-b')
    expect(h.drafts).toEqual([null])

    const pending = h.quoteBubble('run:response:2')
    expect(h.previewCalls).toEqual([])
    expect(h.request.current).toEqual(resetRequest)
    await pending

    h.completeDetailLoad('chat-b')
    expect(h.request.current).toEqual(resetRequest)
    expect(h.drafts).toEqual([null])
  })

  test('预览返回时 selected 已变为 B，即使尚未清理也不能写入 A 的草稿', async () => {
    const h = harness()
    const pending = h.quoteBubble('run:response:2')
    const activeRequest = { ...h.request.current }
    h.selectConversation('chat-b', false)
    expect(h.request.current).toEqual(activeRequest)
    expect(h.drafts).toEqual([])

    h.resolve('run:response:2')
    await pending
    expect(h.drafts).toEqual([])
  })

  test('连续点引用只有最新一次的预览生效', async () => {
    const h = harness()
    const first = h.quoteBubble('run:response:1')
    const second = h.quoteBubble('run:response:2')
    h.resolve('run:response:2')
    await second
    h.resolve('run:response:1')
    await first
    expect(h.drafts).toEqual([preview('run:response:2')])
  })
})