import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { act, cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const { PromptInput, PromptInputTextarea } = await import('../src/components/ai-elements/prompt-input')
const { PROMPT_SEND_SHORTCUT_KEY } = await import('../src/components/ai-elements/prompt-send-shortcut')

afterEach(() => {
  cleanup()
  window.localStorage.clear()
})

/**
 * The submit pipeline converts attachments before it hands the message over, so
 * assertions wait one turn and let React flush the state that follows.
 */
const settled = () => act(async () => {
  await new Promise((resolve) => setTimeout(resolve, 0))
})

/**
 * Mirrors the real composer: the draft is a single controlled state value, the
 * textarea is the only editable control, and `requestSubmit()` activates the
 * submit button exactly as the shipped composer does.
 */
function Composer({ initial = '', onSend }: { initial?: string; onSend: (text: string) => void }) {
  const [draft, setDraft] = React.useState(initial)
  return React.createElement(
    React.Fragment,
    null,
    React.createElement(
      PromptInput,
      {
        onSubmit: (message: { text: string }) => {
          onSend(message.text)
          // The shipped composer clears its draft as soon as it accepts the message.
          setDraft('')
        },
      },
      React.createElement(PromptInputTextarea, {
        value: draft,
        'aria-label': 'prompt',
        onChange: (event: React.ChangeEvent<HTMLTextAreaElement>) => setDraft(event.currentTarget.value),
      }),
      React.createElement('button', { type: 'submit', 'aria-label': 'send' }, 'send'),
    ),
    React.createElement('output', { 'data-testid': 'draft' }, draft),
  )
}

function mount(initial = '') {
  const sent: string[] = []
  render(React.createElement(Composer, { initial, onSend: (text) => sent.push(text) }))
  const textarea = screen.getByLabelText('prompt') as HTMLTextAreaElement
  return { sent, textarea, draft: () => screen.getByTestId('draft').textContent }
}

describe('prompt textarea keyboard behaviour', () => {
  test('Enter sends the current draft', async () => {
    const { sent, textarea } = mount('发送这条消息')
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(sent).toEqual(['发送这条消息']))
  })

  test('Ctrl + Enter breaks the line and never sends, keeping the draft in sync', async () => {
    const { sent, textarea, draft } = mount('hello')
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true })
    expect(textarea.value).toBe('hello\n')
    // The controlled draft must own the newline, otherwise the next re-render
    // would silently drop it.
    expect(draft()).toBe('hello\n')
    await settled()
    expect(sent).toEqual([])
  })

  test('Cmd + Enter behaves like Ctrl + Enter', async () => {
    const { sent, textarea, draft } = mount('hello')
    fireEvent.keyDown(textarea, { key: 'Enter', metaKey: true })
    expect(textarea.value).toBe('hello\n')
    expect(draft()).toBe('hello\n')
    await settled()
    expect(sent).toEqual([])
  })

  test('Shift + Enter still breaks the line', async () => {
    const { sent, textarea, draft } = mount('hello')
    fireEvent.keyDown(textarea, { key: 'Enter', shiftKey: true })
    expect(textarea.value).toBe('hello\n')
    expect(draft()).toBe('hello\n')
    await settled()
    expect(sent).toEqual([])
  })

  test('a newline replaces the selected text and lands the caret after it', async () => {
    const { sent, textarea, draft } = mount('hello')
    textarea.setSelectionRange(1, 4)
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true })
    expect(textarea.value).toBe('h\no')
    expect(draft()).toBe('h\no')
    expect(textarea.selectionStart).toBe(2)
    expect(textarea.selectionEnd).toBe(2)
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
    // The very next plain Enter is a real send again.
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(sent).toEqual(['你好']))
  })

  test('the legacy preference swaps the keys without losing either', async () => {
    window.localStorage.setItem(PROMPT_SEND_SHORTCUT_KEY, 'mod-enter')
    const { sent, textarea, draft } = mount('hello')
    fireEvent.keyDown(textarea, { key: 'Enter' })
    expect(draft()).toBe('hello\n')
    await settled()
    expect(sent).toEqual([])
    textarea.setSelectionRange(textarea.value.length, textarea.value.length)
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true })
    await waitFor(() => expect(sent).toEqual(['hello\n']))
  })

  test('the preference is read per keypress, so changing it applies immediately', async () => {
    const { sent, textarea, draft } = mount('hello')
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true })
    expect(draft()).toBe('hello\n')
    await settled()
    expect(sent).toEqual([])
    window.localStorage.setItem(PROMPT_SEND_SHORTCUT_KEY, 'mod-enter')
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true })
    await waitFor(() => expect(sent).toEqual(['hello\n']))
  })

  test('holding Enter on one draft sends it exactly once', async () => {
    const { sent, textarea } = mount('once')
    fireEvent.keyDown(textarea, { key: 'Enter' })
    fireEvent.keyDown(textarea, { key: 'Enter' })
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await settled()
    expect(sent).toEqual(['once'])
  })

  test('consecutive drafts each send once, without leaking into the next one', async () => {
    const { sent, textarea } = mount('first')
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(sent).toEqual(['first']))
    fireEvent.change(textarea, { target: { value: 'second' } })
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(sent).toEqual(['first', 'second']))
  })

  test('an empty draft never sends an empty message', async () => {
    const { sent, textarea, draft } = mount('')
    fireEvent.keyDown(textarea, { key: 'Enter' })
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true })
    await settled()
    expect(sent).toEqual([])
    expect(draft()).toBe('\n')
  })

  test('a disabled submit button blocks Enter without touching the draft', async () => {
    const sent: string[] = []
    render(React.createElement(
      PromptInput,
      { onSubmit: (message: { text: string }) => sent.push(message.text) },
      React.createElement(PromptInputTextarea, { defaultValue: 'blocked', 'aria-label': 'blocked-prompt' }),
      React.createElement('button', { type: 'submit', 'aria-label': 'blocked-send', disabled: true }, 'send'),
    ))
    const textarea = screen.getByLabelText('blocked-prompt') as HTMLTextAreaElement
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await settled()
    expect(sent).toEqual([])
    expect(textarea.value).toBe('blocked')
  })

  test('Alt + Enter is left to the browser and does not send', async () => {
    const { sent, textarea, draft } = mount('hello')
    fireEvent.keyDown(textarea, { key: 'Enter', altKey: true })
    expect(draft()).toBe('hello')
    await settled()
    expect(sent).toEqual([])
  })
})
