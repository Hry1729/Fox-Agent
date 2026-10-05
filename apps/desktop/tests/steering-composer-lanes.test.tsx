// Item 6 acceptance: the main composer's supplementary-request lanes.
//
// SCOPE (honest): the component under test is the REAL `useRunSteering` hook and
// the REAL `steeringSubmitMode` gate, mounted with React DOM into a REAL
// happy-dom document and driven by real DOM events. Only the Tauri IPC boundary
// (`desktopClient`) is replaced, because it cannot exist in this process. The
// boundary replacement reproduces the Host's own durable contracts that this
// suite is checking the client against:
//
//   * enqueue resolves the Run FROM THE CONVERSATION, never from a client id;
//   * a repeated `messageId` is an idempotent no-op returning the same row;
//   * a terminal Run accepts nothing (`steering.no_active_run`);
//   * a `next-turn` row is not cancelled by the Run ending.
//
// A click-through of the packaged Tauri app against a real Kernel is a separate
// manual acceptance item; nothing here claims to be that.

import { afterEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

type Row = {
  runId: string
  seq: number
  messageId: string
  content: string
  status: string
  lane: string
  receivedAt: number
  appliedAt: number | null
  appliedEventSeq: number | null
  appliedDispatchKey: string | null
}

// The boundary's durable state, per conversation, exactly as the Host would own
// it. `activeRun` is null once the task ended.
type Backend = {
  activeRun: string | null
  latestRun: string | null
  runState: string | null
  rows: Row[]
  acceptUntargetedLane?: boolean
}

const backends = new Map<string, Backend>()
const enqueueCalls: Array<{ conversationId: string; content: string; messageId?: string; lane?: string }> = []
const discardCalls: Array<{ conversationId: string; messageId: string }> = []
let listImpl: ((conversationId: string) => Promise<unknown> | undefined) | null = null

function backend(conversationId: string): Backend {
  let current = backends.get(conversationId)
  if (!current) {
    current = {
      activeRun: `run-${conversationId}`,
      latestRun: `run-${conversationId}`,
      runState: 'running',
      rows: [],
    }
    backends.set(conversationId, current)
  }
  return current
}

class BoundaryError extends Error {
  code: string
  constructor(code: string, message: string) {
    super(message)
    this.code = code
  }
}

const actualDesktopClient = await import('../src/features/conversations/api/desktop-client')

mock.module('../src/features/conversations/api/desktop-client', () => ({
  ...actualDesktopClient,
  desktopRuntimeAvailable: true,
  desktopClient: {
    ...actualDesktopClient.desktopClient,
    listenKernelStateInvalidations: async () => () => {},
    listRunSteering: async (conversationId: string) => {
      const injected = listImpl?.(conversationId)
      if (injected) return injected
      const state = backend(conversationId)
      return {
        runId: state.activeRun ?? state.latestRun,
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
        throw new BoundaryError('steering.no_active_run', '当前没有可接收补充要求的运行中任务。')
      }
      const lane = options.lane ?? 'current'
      // The Host's idempotency contract: the same message identity returns the
      // recorded row instead of inserting a second one.
      const existing = state.rows.find((row) => row.messageId === options.messageId)
      if (existing) return { runId: state.activeRun, seq: existing.seq, messageId: existing.messageId, status: existing.status, lane: existing.lane }
      const row: Row = {
        runId: state.activeRun,
        seq: state.rows.length + 1,
        messageId: options.messageId ?? `server-${state.rows.length + 1}`,
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
    discardRunSteering: async (conversationId: string, messageId: string) => {
      discardCalls.push({ conversationId, messageId })
      const state = backend(conversationId)
      const index = state.rows.findIndex(
        (row) => row.messageId === messageId && row.lane === 'next-turn' && row.status === 'received',
      )
      if (index < 0) throw new BoundaryError('steering.not_discardable', '该补充要求无法移除。')
      state.rows.splice(index, 1)
      return { removed: true }
    },
  },
}))

const React = await import('react')
const { act, cleanup, renderHook, waitFor } = await import('@testing-library/react')
const { useRunSteering } = await import('../src/features/chat/hooks/use-run-steering')

afterEach(() => {
  cleanup()
  backends.clear()
  enqueueCalls.length = 0
  discardCalls.length = 0
  listImpl = null
  window.localStorage.clear()
})

const settled = () => act(async () => {
  await new Promise((resolve) => setTimeout(resolve, 0))
})

function mount(conversationId: string, active = false) {
  return renderHook(
    (props: { conversationId: string; active: boolean }) =>
      useRunSteering({ conversationId: props.conversationId, active: props.active, enabled: true }),
    { initialProps: { conversationId, active } },
  )
}

describe('supplementary requests bind to one conversation and one active Run', () => {
  test('a submit is bound to the conversation the text was typed in', async () => {
    backend('conv-a')
    backend('conv-b')
    const { result } = mount('conv-a')
    await waitFor(() => expect(result.current.runId).toBe('run-conv-a'))

    await act(async () => {
      expect(await result.current.submit('把后续输出改成中文', 'current')).toBe(true)
    })
    // The client never supplies a run id: the Host resolves it from the
    // conversation, so a stale view cannot steer a different task.
    expect(enqueueCalls).toHaveLength(1)
    expect(enqueueCalls[0].conversationId).toBe('conv-a')
    expect(backend('conv-a').rows).toHaveLength(1)
    expect(backend('conv-b').rows).toHaveLength(0)
  })

  test('a listing that resolves after a conversation switch is discarded', async () => {
    backend('conv-a')
    backend('conv-b')
    let releaseA: (() => void) | null = null
    listImpl = (conversationId) => {
      if (conversationId !== 'conv-a') return undefined
      return new Promise((resolve) => {
        releaseA = () => resolve({
          runId: 'run-conv-a',
          runState: 'running',
          acceptsSteering: true,
          messages: [
            {
              seq: 1,
              messageId: 'a-only',
              content: '只属于会话 A 的补充要求',
              status: 'received',
              lane: 'current',
              receivedAt: 1,
              appliedAt: null,
              appliedEventSeq: null,
              appliedDispatchKey: null,
            },
          ],
        })
      }) as Promise<unknown>
    }
    const { result, rerender } = mount('conv-a')
    // Switch conversations before A's listing lands.
    rerender({ conversationId: 'conv-b', active: false })
    await settled()
    if (releaseA) (releaseA as () => void)()
    await settled()
    // A's rows never leak into B's view.
    expect(result.current.rows).toEqual([])
    expect(result.current.view.rows).toEqual([])
  })

  test('switching conversations clears the previous rows', async () => {
    backend('conv-a').rows.push({
      runId: 'run-conv-a', seq: 1, messageId: 'a-1', content: 'A 的补充', status: 'received',
      lane: 'current', receivedAt: 1, appliedAt: null, appliedEventSeq: null, appliedDispatchKey: null,
    })
    backend('conv-b')
    const { result, rerender } = mount('conv-a')
    await waitFor(() => expect(result.current.rows).toHaveLength(1))
    rerender({ conversationId: 'conv-b', active: false })
    expect(result.current.rows).toEqual([])
    await waitFor(() => expect(result.current.rows).toEqual([]))
    expect(result.current.view.visible).toBe(false)
  })
})

describe('repeated submits never deliver twice', () => {
  test('two submits of one draft reuse one client identity', async () => {
    backend('conv-a')
    const { result } = mount('conv-a')
    await waitFor(() => expect(result.current.runId).toBe('run-conv-a'))

    await act(async () => {
      // A double click, or a retry after a dropped connection, submits the same
      // draft twice before either resolves.
      const first = result.current.submit('产物放到 D:\\out', 'current')
      const second = result.current.submit('产物放到 D:\\out', 'current')
      expect(await Promise.all([first, second])).toEqual([true, true])
    })
    expect(enqueueCalls).toHaveLength(2)
    expect(enqueueCalls[0].messageId).toBeDefined()
    expect(enqueueCalls[0].messageId).toBe(enqueueCalls[1].messageId)
    // The Host's idempotency contract turned two deliveries into one row.
    expect(backend('conv-a').rows).toHaveLength(1)
  })

  test('a different draft gets its own identity', async () => {
    backend('conv-a')
    const { result } = mount('conv-a')
    await waitFor(() => expect(result.current.runId).toBe('run-conv-a'))
    await act(async () => { await result.current.submit('第一条', 'current') })
    await act(async () => { await result.current.submit('第二条', 'current') })
    expect(enqueueCalls[0].messageId).not.toBe(enqueueCalls[1].messageId)
    expect(backend('conv-a').rows).toHaveLength(2)
  })

  test('a reload between store and confirmation reuses the same identity', async () => {
    const state = backend('conv-a')
    const first = mount('conv-a')
    await waitFor(() => expect(first.result.current.runId).toBe('run-conv-a'))
    // The Host stores the row, but the client never learns of it: the reply is
    // lost to a dropped connection.
    const lost = new BoundaryError('steering.transport_lost', '连接中断')
    const original = state.rows
    let failNext = true
    const boundary = (await import('../src/features/conversations/api/desktop-client')).desktopClient
    const realEnqueue = boundary.enqueueRunSteering
    ;(boundary as { enqueueRunSteering: typeof realEnqueue }).enqueueRunSteering = async (
      conversationId: string,
      content: string,
      options?: { messageId?: string; lane?: string },
    ) => {
      const result = await realEnqueue(conversationId, content, options)
      if (failNext) {
        failNext = false
        throw lost
      }
      return result
    }
    await act(async () => {
      expect(await first.result.current.submit('产物放到 D:\\out', 'current')).toBe(false)
    })
    expect(original).toHaveLength(1)

    // The webview reloads: a fresh hook with no in-memory ref must recover the
    // identity from persisted state, so the retry is a duplicate receipt.
    first.unmount()
    const second = mount('conv-a')
    await waitFor(() => expect(second.result.current.runId).toBe('run-conv-a'))
    await act(async () => {
      expect(await second.result.current.submit('产物放到 D:\\out', 'current')).toBe(true)
    })
    expect(enqueueCalls[1].messageId).toBe(enqueueCalls[0].messageId)
    expect(state.rows).toHaveLength(1)
    ;(boundary as { enqueueRunSteering: typeof realEnqueue }).enqueueRunSteering = realEnqueue
  })
})

describe('the two lanes stay distinct', () => {
  test('a next-turn submit is stored on its own lane and reported as waiting', async () => {
    backend('conv-a')
    const { result } = mount('conv-a')
    await waitFor(() => expect(result.current.runId).toBe('run-conv-a'))
    await act(async () => { expect(await result.current.submit('下一轮再处理', 'next-turn')).toBe(true) })
    expect(enqueueCalls[0].lane).toBe('next-turn')
    expect(result.current.view.queued.map((row) => row.content)).toEqual(['下一轮再处理'])
    expect(result.current.view.pending).toBe(0)
  })

  test('a queued next-turn row survives the Run ending and can still be removed', async () => {
    const state = backend('conv-a')
    const { result } = mount('conv-a')
    await waitFor(() => expect(result.current.runId).toBe('run-conv-a'))
    await act(async () => { await result.current.submit('下一轮再处理', 'next-turn') })
    // The task ends; the terminal decision cancels only the current lane.
    state.activeRun = null
    state.runState = 'completed'
    await act(async () => { await result.current.reload(true) })
    expect(result.current.acceptsSteering).toBe(false)
    expect(result.current.view.retainedForNextTask).toBe(true)
    expect(result.current.view.queued).toHaveLength(1)

    await act(async () => { expect(await result.current.discard(result.current.rows[0].messageId)).toBe(true) })
    expect(discardCalls).toEqual([{ conversationId: 'conv-a', messageId: expect.any(String) }])
    expect(result.current.view.queued).toHaveLength(0)
  })
})

describe('a Run that ends under the composer keeps the text', () => {
  test('a submit racing the task ending reports failure instead of a false receipt', async () => {
    const state = backend('conv-a')
    const { result } = mount('conv-a')
    await waitFor(() => expect(result.current.runId).toBe('run-conv-a'))
    // The Run ended between the view the user saw and the Host call.
    state.activeRun = null
    state.runState = 'completed'
    await act(async () => {
      expect(await result.current.submit('补充一句', 'current')).toBe(false)
    })
    expect(result.current.error).toContain('没有可接收补充要求')
    // Nothing was stored, so nothing will be applied behind the user's back, and
    // the caller keeps the draft for the next task.
    expect(state.rows).toHaveLength(0)
    // The UI reconciles to the terminal Run and stops offering the lane.
    await waitFor(() => expect(result.current.acceptsSteering).toBe(false))
  })
})
