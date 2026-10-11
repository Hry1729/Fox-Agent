// Hybrid rollback acceptance: the assistant-ui thread (message list) over the
// same external store the LEGACY composer drives.
//
// SCOPE (honest): the REAL `FoxThread` is mounted over a REAL
// `useExternalStoreRuntime`. The legacy composer is not mounted — the hybrid
// link it owns (legacy composer sends → `useDesktopConversation` store →
// ExternalStoreRuntime) is asserted at the store boundary: pushing new
// messages into the runtime's `messages` prop must repaint the assistant-ui
// list, exactly as a legacy-composer send does through the workbench state.

import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { act, cleanup, render, waitFor } = await import('@testing-library/react')
const { AssistantRuntimeProvider, useExternalStoreRuntime } = await import('@assistant-ui/react')
const FoxThread = await import('../src/features/chat/thread/fox-thread')

afterEach(() => {
  cleanup()
  window.localStorage.clear()
})

const flush = () => act(async () => {
  await new Promise((resolve) => setTimeout(resolve, 0))
})

type ThreadMessageLike = Record<string, unknown>

const FOX_PROCESS_DATA = {
  reasoning: '读取项目结构',
  reasoningItems: ['读取项目结构'],
  steps: [{ kind: 'reasoning', id: 'r1', seq: 1, text: '读取项目结构' }],
  tools: [],
  sources: [],
}

const USER_MESSAGE: ThreadMessageLike = {
  role: 'user',
  id: 'u1',
  createdAt: new Date('2026-10-11T09:00:00Z'),
  content: [{ type: 'text', text: '整理这个项目中的文档' }],
}

const ASSISTANT_MESSAGE: ThreadMessageLike = {
  role: 'assistant',
  id: 'a1',
  createdAt: new Date('2026-10-11T09:00:05Z'),
  content: [
    // Normalized data-part shape (what the production converter emits).
    { type: 'data', id: 'run-1', name: 'fox-process', data: FOX_PROCESS_DATA },
    { type: 'text', text: '已完成文档整理' },
  ],
  status: { type: 'complete', reason: 'stop' },
  // Without a `convertMessage` the store rows are used as full ThreadMessages,
  // so the runtime's state getter needs a metadata object.
  metadata: {},
}

function Harness({ messages, onStart }: { messages: ThreadMessageLike[]; onStart?: (suggestion: string) => void }) {
  const runtime = useExternalStoreRuntime({
    messages,
    isRunning: false,
    onNew: () => undefined,
  })
  const fox = {
    events: [],
    eventsByRun: new Map(),
    attachmentNames: new Map(),
    attachmentsByMessage: new Map(),
    knowledgeBindings: [],
    groupHelpers: null,
  }
  return React.createElement(
    AssistantRuntimeProvider,
    { runtime },
    React.createElement(FoxThread.default, { fox, suggestions: ['整理这个项目中的文档'], onStart }),
  )
}

function mount(messages: ThreadMessageLike[], onStart?: (suggestion: string) => void) {
  render(React.createElement(Harness, { messages, onStart }))
}

describe('the assistant-ui thread with the hybrid rollback', () => {
  test('empty conversations show the CLASSIC empty state (mascot + suggestions)', async () => {
    const started: string[] = []
    mount([], (suggestion) => started.push(suggestion))
    await flush()
    expect(document.querySelector('.fox-empty-conversation .fox-empty-heading')).not.toBeNull()
    expect(document.querySelector('.fox-empty-conversation h2')?.textContent).toBe('今天想一起做点什么？')
    const suggestions = [...document.querySelectorAll('.fox-starter-suggestions button')]
    expect(suggestions.length).toBeGreaterThanOrEqual(1)
    // Clicking a suggestion goes through the classic `onStart` (fills the
    // legacy composer draft), not through the runtime's send adapter.
    suggestions[0]!.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await flush()
    expect(started).toEqual(['整理这个项目中的文档'])
    expect(document.querySelector('.fox-thread-composer-form')).toBeNull()
  })

  test('a legacy-composer send (new messages in the store) repaints the assistant-ui list', async () => {
    const { rerender } = render(React.createElement(Harness, { messages: [] }))
    await flush()
    // The optimistic user message the legacy composer paints via
    // `handleComposerPromptCommit` arrives as a new store row.
    rerender(React.createElement(Harness, { messages: [USER_MESSAGE] }))
    await waitFor(() => {
      expect(document.querySelector('.fox-thread-msg.is-user')?.textContent).toContain('整理这个项目中的文档')
    })
    // The assistant reply streams in through the same store.
    rerender(React.createElement(Harness, { messages: [USER_MESSAGE, ASSISTANT_MESSAGE] }))
    await waitFor(() => {
      expect(document.querySelector('.fox-thread-msg.is-assistant')?.textContent).toContain('已完成文档整理')
    })
  })

  test('an assistant message with a process part keeps the avatar ON the process head line', async () => {
    mount([USER_MESSAGE, ASSISTANT_MESSAGE])
    await waitFor(() => {
      expect(document.querySelector('.fox-thread-msg.is-assistant')).not.toBeNull()
    })
    // The avatar shares the process header row (legacy TurnProcessHeader
    // relation), and the old standalone head row is gone for these messages.
    expect(document.querySelector('.fox-runtime-process-head .fox-assistant-avatar')).not.toBeNull()
    expect(document.querySelector('.fox-thread-msg.is-assistant .fox-thread-assistant-head')).toBeNull()
  })

  test('a text-only assistant message still shows the standalone avatar row', async () => {
    const textOnly = {
      role: 'assistant',
      id: 'a2',
      createdAt: new Date('2026-10-11T09:01:00Z'),
      content: [{ type: 'text', text: '纯文本回复' }],
      status: { type: 'complete', reason: 'stop' },
      metadata: {},
    }
    mount([USER_MESSAGE, textOnly])
    await waitFor(() => {
      expect([...document.querySelectorAll('.fox-thread-msg.is-assistant')].some((node) => node.textContent?.includes('纯文本回复')))
    })
    expect(document.querySelector('.fox-thread-msg.is-assistant .fox-thread-assistant-head .fox-assistant-avatar')).not.toBeNull()
  })
})
