// 5D acceptance: the assistant-ui composer's supplementary-request lanes.
//
// SCOPE (honest): the component under test is the REAL `FoxThreadComposer` over
// the REAL assistant-ui primitives, the REAL `useRunSteering` hook and the REAL
// `steeringSubmitMode` gate, mounted with React DOM into a REAL happy-dom
// document and driven by real DOM events. The Tauri IPC boundary
// (`desktopClient`) is replaced with the same durable Host contracts the
// steering-composer-lanes suite documents:
//
//   * enqueue resolves the Run FROM THE CONVERSATION, never from a client id;
//   * a repeated `messageId` is an idempotent no-op returning the same row;
//   * a terminal Run accepts nothing (`steering.no_active_run`).
//
// The thread itself is a REAL external store runtime pinned to `isRunning`, so
// the composer's running state matches a live task.

import { afterEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

// --- the desktop boundary, reproduced durably --------------------------------

type Backend = {
  activeRun: string | null
  runState: string | null
  rows: Array<{ runId: string; seq: number; messageId: string; content: string; status: string; lane: string; receivedAt: number; appliedAt: number | null; appliedEventSeq: number | null; appliedDispatchKey: string | null }>
}

const backends = new Map<string, Backend>()
const enqueueCalls: Array<{ conversationId: string; content: string; messageId?: string; lane?: string }> = []
let seqCounter = 0

function backend(conversationId: string): Backend {
  let current = backends.get(conversationId)
  if (!current) {
    current = { activeRun: `run-${conversationId}`, runState: 'running', rows: [] }
    backends.set(conversationId, current)
  }
  return current
}

const actualDesktopClient = await import('../src/features/conversations/api/desktop-client')

mock.module('../src/features/conversations/api/desktop-client', () => ({
  ...actualDesktopClient,
  desktopRuntimeAvailable: true,
  desktopClient: {
    ...actualDesktopClient.desktopClient,
    listenKernelStateInvalidations: async () => () => {},
    listRunSteering: async (conversationId: string) => {
      const state = backend(conversationId)
      return {
        runId: state.activeRun,
        runState: state.runState,
        acceptsSteering: state.activeRun !== null,
        messages: state.rows.map((row) => ({ ...row })),
      }
    },
    enqueueRunSteering: async (
      conversationId: string,
      content: string,
      options: { messageId?: string; lane?: string } = {},
    ) => {
      enqueueCalls.push({ conversationId, content, ...options })
      const state = backend(conversationId)
      if (state.activeRun === null) {
        throw Object.assign(new Error('当前没有可接收补充要求的运行中任务。'), { code: 'steering.no_active_run' })
      }
      const lane = options.lane ?? 'current'
      const row = {
        runId: state.activeRun,
        seq: ++seqCounter,
        messageId: options.messageId ?? `server-${seqCounter}`,
        content,
        status: 'received',
        lane,
        receivedAt: 1,
        appliedAt: null,
        appliedEventSeq: null,
        appliedDispatchKey: null,
      }
      state.rows.push(row)
      return { runId: state.activeRun, seq: row.seq, messageId: row.messageId, status: row.status, lane: row.lane }
    },
    discardRunSteering: async () => {
      throw Object.assign(new Error('该补充要求无法移除。'), { code: 'steering.not_discardable' })
    },
  },
}))

const React = await import('react')
const { act, cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const { AssistantRuntimeProvider, useExternalStoreRuntime } = await import('@assistant-ui/react')
const { FoxThreadComposer } = await import('../src/features/chat/thread/composer/fox-thread-composer')
const { TooltipProvider } = await import('../src/components/ui/tooltip')

afterEach(() => {
  cleanup()
  backends.clear()
  enqueueCalls.length = 0
  window.localStorage.clear()
})

const settled = () => act(async () => {
  await new Promise((resolve) => setTimeout(resolve, 0))
})

// --- harness -----------------------------------------------------------------

type HarnessOptions = {
  initial?: string
  conversationId?: string
  decisionPending?: boolean
}

function Harness({ initial = '', conversationId = 'conv-a', decisionPending = false }: HarnessOptions) {
  const [draft, setDraft] = React.useState(initial)
  const runtime = useExternalStoreRuntime({
    isRunning: true,
    messages: [],
    onNew: () => {},
  })
  return React.createElement(
    AssistantRuntimeProvider,
    { runtime },
    React.createElement(
      TooltipProvider,
      null,
      React.createElement(FoxThreadComposer, {
        runtimeControlled: true,
        draft,
        setDraft,
        resetKey: 0,
        steeringConversationId: conversationId,
        steeringActive: true,
        runtimeApprovals: decisionPending
          ? [{
              id: 'approval-1', toolCallId: 'call-1', runId: 'run-conv-a', conversationId,
              toolName: 'write_file', status: 'pending', requestedAction: '写入文件',
              request: { kind: 'file_write', path: 'out.txt' }, decision: null,
              requestedAt: 1, resolvedAt: null,
            }]
          : undefined,
        onResolveApproval: () => {},
        onCancel: () => {},
      }),
    ),
  )
}

function mount(options: HarnessOptions = {}) {
  render(React.createElement(Harness, options))
  const textarea = screen.getByRole('textbox') as HTMLTextAreaElement
  return { textarea }
}

describe('the assistant-ui composer steering lanes', () => {
  test('Enter submits the supplementary request to the current task, never Stop', async () => {
    const { textarea } = mount()
    fireEvent.change(textarea, { target: { value: '后续输出改成中文' } })
    await waitFor(() => expect(textarea.value).toBe('后续输出改成中文'))
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(enqueueCalls.map((call) => ({ content: call.content, lane: call.lane }))).toEqual([
      { content: '后续输出改成中文', lane: 'current' },
    ]))
    // The draft was accepted, so the composer releases it.
    await waitFor(() => expect(textarea.value).toBe(''))
  })

  test('an IME composition confirmation never submits, and the next plain Enter does', async () => {
    const { textarea } = mount()
    fireEvent.compositionStart(textarea)
    fireEvent.change(textarea, { target: { value: '把产物放到输出目录' } })
    fireEvent.keyDown(textarea, { key: 'Enter', isComposing: true })
    fireEvent.compositionEnd(textarea, { data: '把产物放到输出目录' })
    await settled()
    expect(enqueueCalls).toEqual([])
    await waitFor(() => expect(textarea.value).toBe('把产物放到输出目录'))

    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(enqueueCalls).toHaveLength(1))
    expect(enqueueCalls[0].content).toBe('把产物放到输出目录')
    expect(enqueueCalls[0].lane).toBe('current')
  })

  test('Ctrl + Enter breaks the line instead of submitting or stopping', async () => {
    const { textarea } = mount({ initial: 'hello' })
    await waitFor(() => expect(textarea.value).toBe('hello'))
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true })
    await waitFor(() => expect(textarea.value).toBe('hello\n'))
    await settled()
    expect(enqueueCalls).toEqual([])
  })

  test('the queue action is never a submit path and never stops the task', async () => {
    const { textarea } = mount({ initial: '放到 D:\\out' })
    await waitFor(() => expect(textarea.value).toBe('放到 D:\\out'))
    // Enter cannot activate the queue button: it is `type="button"`.
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(enqueueCalls).toHaveLength(1))
    expect(enqueueCalls[0].lane).toBe('current')

    // Queue takes the current draft and holds it for the next task.
    fireEvent.change(textarea, { target: { value: '放到 D:\\out' } })
    await waitFor(() => expect(textarea.value).toBe('放到 D:\\out'))
    fireEvent.click(screen.getByTestId('fox-steering-queue'))
    await waitFor(() => expect(enqueueCalls).toHaveLength(2))
    expect(enqueueCalls[1].lane).toBe('next-turn')
    expect(enqueueCalls[1].content).toBe('放到 D:\\out')
  })

  test('Stop is independent: clicking it never submits the draft', async () => {
    mount({ initial: '后续输出改成中文' })
    await waitFor(() => expect(screen.getByTestId('fox-steering-stop')).not.toBeNull())
    fireEvent.click(screen.getByTestId('fox-steering-stop'))
    await settled()
    expect(enqueueCalls).toEqual([])
  })

  test('an empty draft cannot arm either submit lane', async () => {
    mount({ initial: '' })
    await settled()
    const steer = screen.getByTestId('fox-steering-submit') as HTMLButtonElement
    const queue = screen.getByTestId('fox-steering-queue') as HTMLButtonElement
    expect(steer.disabled).toBe(true)
    expect(queue.disabled).toBe(true)
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' })
    await settled()
    expect(enqueueCalls).toEqual([])
  })

  test('a pending decision swaps the lanes out, so a supplement cannot approve it', async () => {
    mount({ decisionPending: true })
    await settled()
    expect(screen.queryByTestId('fox-steering-submit')).toBeNull()
    expect(screen.queryByTestId('fox-steering-queue')).toBeNull()
    // The decision card owns the composer; the stop control remains available.
    expect(screen.queryByTestId('fox-steering-stop')).toBeNull()
    expect(screen.getByLabelText('停止')).not.toBeNull()
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' })
    await settled()
    expect(enqueueCalls).toEqual([])
  })

  test('a Run that no longer accepts steering drops back out of the lanes', async () => {
    mount({ initial: 'hello' })
    await waitFor(() => expect(screen.getByTestId('fox-steering-submit')).not.toBeNull())
    // The task ends: the Host accepts nothing further. A fresh composer (like a
    // re-render after the next listing) reconciles against the terminal Run.
    backend('conv-a').activeRun = null
    backend('conv-a').runState = 'completed'
    cleanup()
    await settled()
    render(React.createElement(Harness, { initial: 'hello' }))
    await settled()
    expect(screen.queryByTestId('fox-steering-submit')).toBeNull()
    expect(screen.queryByTestId('fox-steering-queue')).toBeNull()
  })
})
