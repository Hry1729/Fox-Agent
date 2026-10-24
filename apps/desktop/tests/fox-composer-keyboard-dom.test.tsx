// 5D acceptance: the assistant-ui composer (FoxThreadComposer) keyboard rules.
//
// SCOPE (honest): the REAL `FoxThreadComposer` is mounted over the REAL
// assistant-ui primitives (`ComposerPrimitive.Root/Input`) and a REAL external
// store runtime (`useExternalStoreRuntime` with an empty message list), driven
// by real DOM events in a REAL happy-dom document. The Tauri conversation
// runtime behind `useFoxRuntime` is replaced by the external store adapter —
// that boundary's send path is asserted through the adapter's `onNew` calls.

import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { act, cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const { AssistantRuntimeProvider, useExternalStoreRuntime } = await import('@assistant-ui/react')
const { FoxThreadComposer } = await import('../src/features/chat/thread/composer/fox-thread-composer')
const { TooltipProvider } = await import('../src/components/ui/tooltip')

afterEach(() => {
  cleanup()
  window.localStorage.clear()
})

/** The submit pipeline resolves through the runtime; assertions flush one turn. */
const settled = () => act(async () => {
  await new Promise((resolve) => setTimeout(resolve, 0))
})

type AppendMessageLike = { content: Array<{ type: string; text?: string }> }

/**
 * Mirrors the shipped composer: the draft is a single controlled state value
 * owned by the host, and the runtime owns the text. `onNew` is the send path.
 */
function Harness({ initial = '', onSend }: { initial?: string; onSend: (text: string) => void }) {
  const [draft, setDraft] = React.useState(initial)
  const runtime = useExternalStoreRuntime({
    isRunning: false,
    messages: [],
    onNew: (message: AppendMessageLike) => {
      const text = message.content
        .filter((part) => part.type === 'text')
        .map((part) => part.text ?? '')
        .join('')
      onSend(text)
      setDraft('')
    },
  })
  return React.createElement(
    React.Fragment,
    null,
    React.createElement(
      AssistantRuntimeProvider,
      { runtime },
      React.createElement(TooltipProvider, null, React.createElement(FoxThreadComposer, {
        runtimeControlled: false,
        draft,
        setDraft,
        resetKey: 0,
      })),
    ),
    React.createElement('output', { 'data-testid': 'draft' }, draft),
  )
}

function mount(initial = '') {
  const sent: string[] = []
  render(React.createElement(Harness, { initial, onSend: (text) => sent.push(text) }))
  const textarea = screen.getByRole('textbox') as HTMLTextAreaElement
  return { sent, textarea, draft: () => screen.getByTestId('draft').textContent }
}

describe('the assistant-ui composer keyboard behaviour', () => {
  test('Enter sends the current draft through the runtime', async () => {
    const { sent, textarea } = mount('发送这条消息')
    await waitFor(() => expect(textarea.value).toBe('发送这条消息'))
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(sent).toEqual(['发送这条消息']))
  })

  test('Ctrl + Enter breaks the line and never sends, keeping the draft in sync', async () => {
    const { sent, textarea, draft } = mount('hello')
    await waitFor(() => expect(textarea.value).toBe('hello'))
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true })
    await waitFor(() => expect(textarea.value).toBe('hello\n'))
    // The controlled draft must own the newline, otherwise the next re-render
    // would silently drop it.
    expect(draft()).toBe('hello\n')
    await settled()
    expect(sent).toEqual([])
  })

  test('Cmd + Enter behaves like Ctrl + Enter', async () => {
    const { sent, textarea, draft } = mount('hello')
    await waitFor(() => expect(textarea.value).toBe('hello'))
    fireEvent.keyDown(textarea, { key: 'Enter', metaKey: true })
    await waitFor(() => expect(textarea.value).toBe('hello\n'))
    expect(draft()).toBe('hello\n')
    await settled()
    expect(sent).toEqual([])
  })

  test('Shift + Enter still breaks the line', async () => {
    const { sent, textarea, draft } = mount('hello')
    await waitFor(() => expect(textarea.value).toBe('hello'))
    fireEvent.keyDown(textarea, { key: 'Enter', shiftKey: true })
    await waitFor(() => expect(textarea.value).toBe('hello\n'))
    expect(draft()).toBe('hello\n')
    await settled()
    expect(sent).toEqual([])
  })

  test('a newline replaces the selected text and lands the caret after it', async () => {
    const { sent, textarea, draft } = mount('hello')
    await waitFor(() => expect(textarea.value).toBe('hello'))
    textarea.setSelectionRange(1, 4)
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true })
    await waitFor(() => expect(textarea.value).toBe('h\no'))
    expect(draft()).toBe('h\no')
    await waitFor(() => {
      expect(textarea.selectionStart).toBe(2)
      expect(textarea.selectionEnd).toBe(2)
    })
    await settled()
    expect(sent).toEqual([])
  })

  test('an IME confirmation of a candidate never sends', async () => {
    const { sent, textarea, draft } = mount('')
    fireEvent.compositionStart(textarea)
    fireEvent.change(textarea, { target: { value: '你好' } })
    fireEvent.keyDown(textarea, { key: 'Enter', isComposing: true })
    fireEvent.compositionEnd(textarea, { data: '你好' })
    await settled()
    expect(sent).toEqual([])
    expect(draft()).toBe('你好')
    await waitFor(() => expect(textarea.value).toBe('你好'))
    // The very next plain Enter is a real send again.
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(sent).toEqual(['你好']))
  })

  test('a legacy keyCode 229 confirmation never sends either', async () => {
    const { sent, textarea } = mount('你好')
    await waitFor(() => expect(textarea.value).toBe('你好'))
    fireEvent.keyDown(textarea, { key: 'Enter', keyCode: 229 })
    await settled()
    expect(sent).toEqual([])
  })

  test('Ctrl/Cmd + Enter mode: Enter breaks the line and Ctrl + Enter sends', async () => {
    window.localStorage.setItem('fox.preferences.sendKey', 'mod-enter')
    const { sent, textarea, draft } = mount('hello')
    await waitFor(() => expect(textarea.value).toBe('hello'))
    textarea.setSelectionRange(textarea.value.length, textarea.value.length)
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(draft()).toBe('hello\n'))
    await settled()
    expect(sent).toEqual([])
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true })
    await waitFor(() => expect(sent).toEqual(['hello\n']))
  })

  test('Ctrl/Cmd + Enter mode: an IME confirmation never sends', async () => {
    window.localStorage.setItem('fox.preferences.sendKey', 'mod-enter')
    const { sent, textarea } = mount('你好')
    await waitFor(() => expect(textarea.value).toBe('你好'))
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true, isComposing: true })
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true, keyCode: 229 })
    fireEvent.keyDown(textarea, { key: 'Enter', keyCode: 229 })
    await settled()
    expect(sent).toEqual([])
  })

  test('Alt + Enter is left to the browser and does not send', async () => {
    const { sent, textarea, draft } = mount('hello')
    await waitFor(() => expect(textarea.value).toBe('hello'))
    fireEvent.keyDown(textarea, { key: 'Enter', altKey: true })
    await settled()
    expect(draft()).toBe('hello')
    expect(sent).toEqual([])
  })

  test('an empty draft never sends an empty message', async () => {
    const { sent } = mount('')
    await settled()
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' })
    await settled()
    expect(sent).toEqual([])
  })

  test('the send button is disabled while the draft is empty', async () => {
    mount('')
    await settled()
    const send = screen.getByLabelText('发送') as HTMLButtonElement
    expect(send.disabled).toBe(true)
    fireEvent.change(screen.getByRole('textbox'), { target: { value: '有了' } })
    await waitFor(() => expect((screen.getByLabelText('发送') as HTMLButtonElement).disabled).toBe(false))
  })
})
