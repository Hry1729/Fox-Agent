// Item 6 acceptance (DOM): while a task is running, the main composer submits
// supplementary requests — and Enter can never reach the Stop control.
//
// SCOPE (honest): the REAL `PromptInput` form primitives, the REAL
// `PromptInputTextarea` (with its real IME composition guard) and the REAL
// `steeringSubmitMode` gate are mounted into a REAL happy-dom document. The
// control set below mirrors the shipped composer's steering mode: a
// `type="submit"` primary action for the current task, a `type="button"` queue
// action, and a separate `type="button"` Stop. The full `workbench.tsx` Composer
// is not mountable in isolation (it owns the conversation runtime), so its
// steering-control structure is reproduced here rather than claimed to be
// executed.

import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const { PromptInput, PromptInputTextarea } = await import('../src/components/ai-elements/prompt-input')
const { steeringSubmitMode } = await import('../src/features/chat/components/steering-presentation')

afterEach(() => {
  cleanup()
  window.localStorage.clear()
})

const settled = () => new Promise((resolve) => setTimeout(resolve, 0))

/** The shipped steering-mode control set, driven by the real gate. */
function SteeringComposer({
  initial = '',
  acceptsSteering = true,
  decisionPending = false,
  onSteer,
  onQueue,
  onStop,
}: {
  initial?: string
  acceptsSteering?: boolean
  decisionPending?: boolean
  onSteer: (text: string) => void
  onQueue: (text: string) => void
  onStop: () => void
}) {
  const [draft, setDraft] = React.useState(initial)
  const steeringMode = steeringSubmitMode({
    runtimeControlled: true,
    status: 'streaming',
    acceptsSteering,
    decisionPending,
    hasActiveQuestion: false,
  })
  return React.createElement(
    PromptInput,
    {
      onSubmit: (message: { text: string }) => {
        onSteer(message.text)
        setDraft('')
      },
    },
    React.createElement(PromptInputTextarea, {
      value: draft,
      'aria-label': 'prompt',
      onChange: (event: React.ChangeEvent<HTMLTextAreaElement>) => setDraft(event.currentTarget.value),
    }),
    steeringMode
      ? React.createElement(
          React.Fragment,
          null,
          React.createElement('button', { type: 'submit', 'data-testid': 'steer', disabled: !draft.trim() }, '补充到当前任务'),
          React.createElement('button', { type: 'button', 'data-testid': 'queue', onClick: () => onQueue(draft) }, '排队，下一轮处理'),
          React.createElement('button', { type: 'button', 'data-testid': 'stop', onClick: onStop }, '停止'),
        )
      : React.createElement('button', { type: 'submit', 'data-testid': 'send' }, '发送'),
  )
}

function mount(options: Partial<{ initial: string; acceptsSteering: boolean; decisionPending: boolean }> = {}) {
  const steered: string[] = []
  const queued: string[] = []
  let stops = 0
  render(
    React.createElement(SteeringComposer, {
      initial: options.initial ?? '后续输出改成中文',
      acceptsSteering: options.acceptsSteering ?? true,
      decisionPending: options.decisionPending ?? false,
      onSteer: (text) => steered.push(text),
      onQueue: (text) => queued.push(text),
      onStop: () => { stops += 1 },
    }),
  )
  return {
    steered,
    queued,
    stops: () => stops,
    textarea: screen.getByLabelText('prompt') as HTMLTextAreaElement,
  }
}

describe('running-task composer', () => {
  test('Enter submits the supplementary request to the current task, never Stop', async () => {
    const { steered, stops, textarea } = mount()
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(steered).toEqual(['后续输出改成中文']))
    // The key reached the steering submit button, which is the form's default
    // submit; Stop is a `type="button"` and cannot be activated by Enter.
    expect(stops()).toBe(0)
  })

  test('an IME composition confirmation never sends, and the next plain Enter does', async () => {
    const { steered, stops, textarea } = mount({ initial: '' })
    fireEvent.compositionStart(textarea)
    fireEvent.change(textarea, { target: { value: '把产物放到输出目录' } })
    fireEvent.keyDown(textarea, { key: 'Enter', isComposing: true })
    fireEvent.compositionEnd(textarea, { data: '把产物放到输出目录' })
    await settled()
    expect(steered).toEqual([])
    expect(stops()).toBe(0)
    expect(textarea.value).toBe('把产物放到输出目录')

    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(steered).toEqual(['把产物放到输出目录']))
    expect(stops()).toBe(0)
  })

  test('a legacy keyCode 229 confirmation never sends either', async () => {
    const { steered, stops, textarea } = mount({ initial: '你好' })
    fireEvent.keyDown(textarea, { key: 'Enter', keyCode: 229 })
    await settled()
    expect(steered).toEqual([])
    expect(stops()).toBe(0)
  })

  test('Ctrl + Enter breaks the line instead of submitting or stopping', async () => {
    const { steered, stops, textarea } = mount({ initial: 'hello' })
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true })
    await settled()
    expect(textarea.value).toBe('hello\n')
    expect(steered).toEqual([])
    expect(stops()).toBe(0)
  })

  test('the queue action is never a submit path and never stops the task', async () => {
    const { steered, queued, stops, textarea } = mount({ initial: '放到 D:\\out' })
    // Enter cannot activate the queue button: it is `type="button"`.
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(steered).toEqual(['放到 D:\\out']))
    expect(queued).toEqual([])
    fireEvent.click(screen.getByTestId('queue'))
    expect(queued).toEqual([''])
    expect(stops()).toBe(0)
  })

  test('Stop is independent: clicking it never submits the draft', async () => {
    const { steered, queued, stops } = mount()
    fireEvent.click(screen.getByTestId('stop'))
    await settled()
    expect(stops()).toBe(1)
    expect(steered).toEqual([])
    expect(queued).toEqual([])
  })

  test('an empty draft cannot arm either submit lane', async () => {
    const { steered, stops, textarea } = mount({ initial: '' })
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await settled()
    expect(steered).toEqual([])
    expect(stops()).toBe(0)
    expect((screen.getByTestId('steer') as HTMLButtonElement).disabled).toBe(true)
  })

  test('a pending decision swaps the lanes out, so a supplement cannot approve it', async () => {
    mount({ decisionPending: true })
    expect(screen.queryByTestId('steer')).toBeNull()
    expect(screen.queryByTestId('queue')).toBeNull()
    expect(screen.queryByTestId('stop')).toBeNull()
    expect(screen.getByTestId('send')).not.toBeNull()
  })

  test('a Run that no longer accepts steering drops back to a normal send', async () => {
    mount({ acceptsSteering: false })
    expect(screen.queryByTestId('steer')).toBeNull()
    expect(screen.queryByTestId('stop')).toBeNull()
    expect(screen.getByTestId('send')).not.toBeNull()
  })
})
