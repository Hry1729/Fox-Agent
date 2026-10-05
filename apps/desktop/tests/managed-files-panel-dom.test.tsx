// Real-DOM interaction acceptance for the two user decision surfaces that carry
// a request / approval identity:
//
//   1. `ManagedFilesPanel` — the restore button ("恢复" / "强制恢复") and the
//      restore request identity the panel hands to the Host.
//   2. `RuntimeApprovalPrompt` — the whole-file replacement approval and the
//      approval identity it submits on confirm / deny.
//
// SCOPE (honest): the component under test, its event handlers, its presentation
// rules and the request-identity module are the REAL ones, mounted with React DOM
// into a REAL happy-dom document, driven by real DOM click events. Only the Tauri
// IPC boundary (`desktopClient`) and the in-app notification side effect are
// mocked, because neither can exist in this process. `apps/desktop/tests/`
// already mocks that same boundary in `managed-files-panel-restore.test.tsx`.
//
// A click-through of the packaged Tauri desktop app is still a separate manual
// acceptance item; nothing here claims to be that.

import { afterEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

// Several DOM suites share one process, so the first one to run owns registration.
// Registering twice throws and aborts this file's setup mid-run.
if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

type RestoreCall = {
  conversationId: string
  versionId: string
  force: boolean
  requestId: string | null
}

const restoreCalls: RestoreCall[] = []
let restoreImpl: (call: RestoreCall) => Promise<unknown> = async () => ({
  id: 'v-restored',
  versionNo: 9,
})

// The Host's own dedup contract: a repeated delivery of the SAME request identity
// reads the recorded result instead of mutating the file again. Reproduced at the
// boundary so the panel is held to it, without inventing Host behaviour.
const settledByRequestId = new Map<string, unknown>()
const dedupAtBoundary = (call: RestoreCall) => {
  if (call.requestId && settledByRequestId.has(call.requestId)) {
    return settledByRequestId.get(call.requestId)
  }
  return undefined
}

const versionRow = (overrides: Record<string, unknown>) => ({
  id: 'v1',
  runId: 'run-1',
  toolCallId: 'call-1',
  tool: 'write_file',
  storagePath: 'C:/tmp/a.txt',
  displayName: 'a.txt',
  versionNo: 1,
  changeKind: 'modified',
  beforeHash: null,
  beforeSize: null,
  afterHash: 'hash-1',
  afterSize: 10,
  backupPath: 'C:/tmp/backup-1',
  restoredFromId: null,
  createdAt: 1,
  backupAvailable: true,
  currentHash: 'hash-2',
  drifted: false,
  restoreSize: 10,
  canRestore: true,
  sourceKind: 'host_capture',
  restoreBlocker: null,
  ...overrides,
})

// Newest first, as the Host returns them. v2 is the file's current content, so
// only v1 offers a restore action.
let versions: unknown[] = [
  versionRow({ id: 'v2', versionNo: 2, createdAt: 2, currentHash: 'hash-2' }),
  versionRow({ id: 'v1', versionNo: 1, createdAt: 1 }),
]

// The real modules are loaded first so the mocks below can SPREAD them: a
// `mock.module` factory replaces the module for the whole process, and a partial
// factory would break every other test file that imports the same module.
const actualDesktopClient = await import(
  '../src/features/conversations/api/desktop-client'
)
const actualNotifications = await import('../src/features/notifications')

mock.module('../src/features/conversations/api/desktop-client', () => ({
  ...actualDesktopClient,
  desktopRuntimeAvailable: true,
  desktopClient: {
    ...actualDesktopClient.desktopClient,
    listManagedFileVersions: async () => versions,
    restoreManagedFileVersion: async (
      conversationId: string,
      versionId: string,
      force = false,
      requestId?: string,
    ) => {
      const call: RestoreCall = {
        conversationId,
        versionId,
        force,
        requestId: requestId ?? null,
      }
      restoreCalls.push(call)
      const recorded = dedupAtBoundary(call)
      if (recorded !== undefined) return recorded
      const result = await restoreImpl(call)
      if (call.requestId) settledByRequestId.set(call.requestId, result)
      return result
    },
    listenKernelStateInvalidations: async () => () => {},
  },
}))

mock.module('../src/features/notifications', () => ({
  ...actualNotifications,
  notify: Object.assign(() => {}, {
    success: () => {},
    error: () => {},
    info: () => {},
    warning: () => {},
  }),
}))

const React = await import('react')
const { cleanup, fireEvent, render, screen, waitFor } = await import(
  '@testing-library/react'
)
const { ManagedFilesPanel } = await import(
  '../src/features/chat/components/ManagedFilesPanel'
)
const { RuntimeApprovalPrompt } = await import(
  '../src/features/chat/components/RuntimeApprovalPrompt'
)

afterEach(() => {
  cleanup()
  restoreCalls.length = 0
  settledByRequestId.clear()
  restoreImpl = async () => ({ id: 'v-restored', versionNo: 9 })
  versions = [
    versionRow({ id: 'v2', versionNo: 2, createdAt: 2, currentHash: 'hash-2' }),
    versionRow({ id: 'v1', versionNo: 1, createdAt: 1 }),
  ]
  ;(globalThis as any).window.confirm = () => true
})

/** Mount the real panel and expand its real header button. */
async function mountPanel() {
  ;(globalThis as any).window.confirm = () => true
  render(React.createElement(ManagedFilesPanel, { conversationId: 'conv-1' }))
  const header = await screen.findByRole('button', { name: /文件版本恢复/ })
  fireEvent.click(header)
  return header
}

const restoreButton = () => screen.getByRole('button', { name: '恢复' })

describe('ManagedFilesPanel render contract', () => {
  // Server rendering never runs `useEffect`, so the version list is still empty
  // and the panel takes its documented early return. This covers the "no
  // interactive surface until the Host has answered" rule only; the affordances
  // themselves are covered by the real-DOM clicks below.
  test('the panel renders no surface before the Host has answered', async () => {
    const { renderToStaticMarkup } = await import('react-dom/server')
    const html = renderToStaticMarkup(
      React.createElement(ManagedFilesPanel, { conversationId: 'conv-1' }),
    )
    expect(html).toBe('')
  })
})

describe('ManagedFilesPanel restore button (real DOM clicks)', () => {
  test('a fast double click produces exactly one restore request', async () => {
    // Undetermined on purpose: the first request is still in flight.
    restoreImpl = () => new Promise(() => {})
    await mountPanel()

    const button = restoreButton()
    fireEvent.click(button)
    // The panel disabled the action while it is undetermined; a second click in
    // the same double click must not become a second execution request.
    fireEvent.click(button)

    expect(restoreCalls).toHaveLength(1)
    expect(restoreCalls[0].conversationId).toBe('conv-1')
    expect(restoreCalls[0].versionId).toBe('v1')
    expect(restoreCalls[0].force).toBe(false)
    expect(typeof restoreCalls[0].requestId).toBe('string')
    expect(restoreCalls[0].requestId).not.toBe('')
  })

  test('a resend after a timed-out request keeps the same logical identity', async () => {
    // 1. A drift rejection turns the row's action into its force form.
    restoreImpl = async () => {
      throw new Error('a.txt changed outside the recorded task history')
    }
    await mountPanel()
    fireEvent.click(restoreButton())
    const forceButton = await screen.findByRole('button', { name: /强制恢复/ })

    // 2. The force confirmation is delivered but never answered: the client gives
    //    up on the wait, so the outcome is UNDETERMINED, not a refusal. The Host
    //    may already have recorded this request identity.
    restoreImpl = async () => {
      throw new Error('request timed out')
    }
    fireEvent.click(forceButton)
    await waitFor(() => expect(restoreCalls).toHaveLength(2))
    const firstForceId = restoreCalls[1].requestId
    expect(restoreCalls[1].force).toBe(true)
    // The failed wait re-opened the affordance instead of wedging the row.
    const resendButton = await screen.findByRole('button', { name: /强制恢复/ })

    // 3. The resend must carry the SAME identity, so the Host recognizes one
    //    logical request instead of writing the file twice.
    fireEvent.click(resendButton)
    await waitFor(() => expect(restoreCalls).toHaveLength(3))
    expect(restoreCalls[2].requestId).toBe(firstForceId)
    expect(restoreCalls[2].force).toBe(true)

    // Exactly two logical actions: the rejected normal one and the forced one.
    expect(new Set(restoreCalls.map((call) => call.requestId)).size).toBe(2)
  })

  test('a new action after a definite success mints a new identity', async () => {
    await mountPanel()
    fireEvent.click(restoreButton())
    await waitFor(() => expect(restoreCalls).toHaveLength(1))

    // The first action ended definitely (the Host recorded a result), so the
    // identity is released and the next confirmation is a NEW action.
    await waitFor(() => expect(restoreButton().hasAttribute('disabled')).toBe(false))
    fireEvent.click(restoreButton())
    await waitFor(() => expect(restoreCalls).toHaveLength(2))

    expect(restoreCalls[0].requestId).not.toBe(restoreCalls[1].requestId)
    expect(restoreCalls.every((call) => call.force === false)).toBe(true)
  })

  test('turning a normal restore into a forced one never reuses the identity', async () => {
    restoreImpl = async () => {
      throw new Error('a.txt changed outside the recorded task history')
    }
    await mountPanel()
    fireEvent.click(restoreButton())
    await waitFor(() => expect(restoreCalls).toHaveLength(1))

    // The rejection is shown and the force affordance appears.
    const forceButton = await screen.findByRole('button', { name: /强制恢复/ })
    expect(document.querySelector('.fox-managed-error')?.textContent).toContain(
      'changed outside the recorded task history',
    )
    expect(document.querySelector('.fox-managed-force-hint')?.textContent).toContain(
      'changed outside the recorded task history',
    )

    restoreImpl = async () => ({ id: 'v-restored', versionNo: 9 })
    fireEvent.click(forceButton)
    await waitFor(() => expect(restoreCalls).toHaveLength(2))

    // A forced overwrite carries different parameters, so it must not inherit the
    // normal action's identity: the Host binds a request to exactly one intent.
    expect(restoreCalls[0].force).toBe(false)
    expect(restoreCalls[1].force).toBe(true)
    expect(restoreCalls[1].requestId).not.toBe(restoreCalls[0].requestId)
  })
})

const replacementRequest = {
  category: 'tool_execution',
  availableDecisions: ['allow_once', 'deny'],
  title: '允许 Fox 整文件替换 a.txt？',
  target: 'C:/tmp/a.txt',
  summary: '写入整个文件内容',
  input: { path: 'C:/tmp/a.txt', content: 'whole file' },
  wholeFileReplacement: {
    purpose: '整文件替换',
    requestDigest: 'sha256:req-abc',
    targetIdentity: 'C:/tmp/a.txt',
    baselineVersion: 'sha256:base-1',
    candidateDigest: 'sha256:cand-2',
  },
}

const approvalRecord = (overrides: Record<string, unknown> = {}) => ({
  // The Kernel mints a VERSIONED approval ticket; the Host re-checks that exact
  // string in the transaction that consumes it.
  id: 'kernel-approval:run-1:write_file:v7',
  toolCallId: 'write-1',
  runId: 'run-1',
  conversationId: 'conv-1',
  toolName: 'write_file',
  status: 'pending',
  requestedAction: 'write_file',
  request: replacementRequest,
  decision: null,
  requestedAt: 1,
  resolvedAt: null,
  ...overrides,
})

const approvalTicket = 'kernel-approval:run-1:write_file:v7'

describe('RuntimeApprovalPrompt (real DOM clicks)', () => {
  test('shows the real purpose and the immutable replacement binding', async () => {
    render(
      React.createElement(RuntimeApprovalPrompt, {
        approval: approvalRecord() as any,
        onResolve: async () => true,
      }),
    )

    const binding = document.querySelector('.fox-approval-replacement')
    expect(binding).not.toBeNull()
    const text = binding!.textContent ?? ''
    expect(text).toContain('整文件替换')
    expect(text).toContain('这不是普通写入审批')
    expect(text).toContain('sha256:req-abc')
    expect(text).toContain('C:/tmp/a.txt')
    expect(text).toContain('sha256:base-1')
    expect(text).toContain('sha256:cand-2')

    // A replacement approval authorizes exactly one dispatch, so the reusable
    // "allow this whole conversation" choice must not be offered.
    expect(screen.queryByRole('button', { name: /本次(?:运行|对话)内允许/ })).toBeNull()
    expect(screen.getByRole('button', { name: '拒绝' })).toBeDefined()
    expect(screen.getByRole('button', { name: '只允许这一次' })).toBeDefined()
  })

  test('an ordinary approval exposes the conversation-scoped reuse and its real limits', async () => {
    render(
      React.createElement(RuntimeApprovalPrompt, {
        approval: approvalRecord({
          request: {
            category: 'tool_execution',
            availableDecisions: ['allow_once', 'deny', 'allow_conversation'],
            title: '允许 Fox 运行命令？',
            target: 'npm test',
            summary: '运行测试',
          },
        }) as any,
        onResolve: async () => true,
      }),
    )

    expect(document.querySelector('.fox-approval-replacement')).toBeNull()
    // The grant is stored against the conversation (and is revoked when the
    // policy generation moves), so the label must not claim a narrower lifetime.
    expect(screen.getByRole('button', { name: '本次对话内允许' })).toBeDefined()
    // The user is told the real scope, that it is revocable, and that revoking or
    // changing the mode ends it — instead of an unqualified "allow this run".
    expect(document.querySelector('.fox-approval-scope-note')?.textContent).toContain('可随时撤销')
    expect(document.querySelector('.fox-approval-scope-note')?.textContent).toContain('立即失效')
    expect(document.querySelector('.fox-approval-scope-note')?.textContent).toContain('不包含执行任何脚本或命令')
  })

  test('a new file is described as a creation, not as a one-dispatch replacement', async () => {
    render(
      React.createElement(RuntimeApprovalPrompt, {
        approval: approvalRecord({
          request: {
            category: 'tool_execution',
            availableDecisions: ['allow_once', 'allow_conversation', 'deny'],
            title: '允许 Fox 写入新文件？',
            target: 'C:/tmp/notes.md',
            input: { path: 'C:/tmp/notes.md', content: 'hello\n' },
            // A legacy card may still carry the binding with the absent-target
            // baseline; it must never be read as a replacement ticket.
            wholeFileReplacement: {
              ...replacementRequest.wholeFileReplacement,
              baselineVersion: 'missing',
            },
          },
        }) as any,
        onResolve: async () => true,
      }),
    )

    const scope = document.querySelector('.fox-approval-scope-note')?.textContent ?? ''
    expect(scope).toContain('新建')
    expect(scope).toContain('不覆盖任何已有内容')
    expect(scope).not.toContain('仅授权本次操作')
    // Nothing existing is destroyed, so the conversation-scoped decision the Host
    // declared stays available.
    expect(screen.getByRole('button', { name: '本次对话内允许' })).toBeDefined()
    expect(screen.getByRole('button', { name: '只允许这一次' })).toBeDefined()
    expect(screen.getByRole('button', { name: '拒绝' })).toBeDefined()
  })

  test('confirm and deny submit the exact versioned approval identity', async () => {
    const resolved: Array<[string, string]> = []
    render(
      React.createElement(RuntimeApprovalPrompt, {
        approval: approvalRecord() as any,
        onResolve: async (approvalId: string, decision: string) => {
          resolved.push([approvalId, decision])
          return true
        },
      }),
    )

    fireEvent.click(screen.getByRole('button', { name: '只允许这一次' }))
    await waitFor(() => expect(resolved).toHaveLength(1))
    expect(resolved[0]).toEqual([approvalTicket, 'allow_once'])

    cleanup()
    resolved.length = 0
    render(
      React.createElement(RuntimeApprovalPrompt, {
        approval: approvalRecord() as any,
        onResolve: async (approvalId: string, decision: string) => {
          resolved.push([approvalId, decision])
          return true
        },
      }),
    )
    fireEvent.click(screen.getByRole('button', { name: '拒绝' }))
    await waitFor(() => expect(resolved).toHaveLength(1))
    expect(resolved[0]).toEqual([approvalTicket, 'deny'])
  })

  test('the surface locks while submitting and re-opens when delivery fails', async () => {
    let release: (value: boolean) => void = () => {}
    const pending = new Promise<boolean>((resolve) => {
      release = resolve
    })
    render(
      React.createElement(RuntimeApprovalPrompt, {
        approval: approvalRecord() as any,
        onResolve: () => pending,
      }),
    )

    const allow = screen.getByRole('button', { name: '只允许这一次' })
    const deny = screen.getByRole('button', { name: '拒绝' })
    fireEvent.click(allow)

    // While the decision is undetermined the user cannot submit a second one.
    await waitFor(() => expect(allow.hasAttribute('disabled')).toBe(true))
    expect(deny.hasAttribute('disabled')).toBe(true)
    fireEvent.click(deny)
    fireEvent.click(allow)

    // Delivery failed: the approval is still pending, so the surface re-opens.
    release(false)
    await waitFor(() => expect(allow.hasAttribute('disabled')).toBe(false))
    expect(deny.hasAttribute('disabled')).toBe(false)
  })
})
