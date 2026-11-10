// D1-6 nit：侧栏等待指示与详情共用同一份单调合并结果。
//
// 真实的 useDesktopConversation 挂在 happy-dom 上，approval 通道按真实契约
// 投递事件序列：pending 请求 → 失效结算事件 → 迟到的不带屏障 pending 重放。
// 迟到的旧事件不得把已失效批准的「等你处理」指示重新点亮，详情里的持久原因
// 也不能回退；另一条合法 pending 批准仍正常点亮指示。

import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const actualDesktopClient = await import('../src/features/conversations/api/desktop-client')
const realDesktopClient = { ...actualDesktopClient }

const approvalHandlers: Array<(approval: any) => void> = []
const summary = {
  id: 'chat', title: '联系人', agentId: 'fox-general', agentBackend: 'fox',
  activeRunId: 'run', activeRunStatus: 'running', awaitingApproval: false,
  lastRunId: 'run', lastRunStatus: 'running', awaitingReply: false,
  updatedAt: 1700000000000, createdAt: 1700000000000, lastMessageAt: null,
  archived: false, status: 'active', projectId: null, projectRoot: null,
}
const persisted = {
  conversation: { id: 'chat', agentId: 'fox-general', agentName: 'Fox', title: '联系人', projectId: null, projectRoot: null,
    permissionMode: 'ask', status: 'active', createdAt: 0, updatedAt: 0, lastMessageAt: null },
  companionId: 'contact', messages: [], runtimeEvents: [], toolCalls: [], approvals: [] as any[], attachments: [], artifacts: [],
  knowledgeBindings: [], expertBindings: [], hasEarlierMessages: false, goals: [], tasks: [], evidence: [],
  planRevisions: [], reviewFindings: [], acceptances: [], childRuns: [], lastRun: null,
}

mock.module('../src/features/conversations/api/desktop-client', () => ({
  ...realDesktopClient,
  desktopRuntimeAvailable: true,
  desktopClient: {
    ...realDesktopClient.desktopClient,
    initialize: async () => ({ defaultAgentId: 'fox-general' }),
    runtimeStatus: async () => ({}),
    listConversations: async () => [structuredClone(summary)],
    listArchivedConversations: async () => [],
    listTrashedConversations: async () => [],
    loadConversation: async () => structuredClone(persisted),
    agentBackendsStatus: async () => null,
    agentBackendPreferences: async () => null,
    recordUiMetric: async () => {},
    listenApprovalRequests: async (handler: (approval: any) => void) => { approvalHandlers.push(handler); return () => {} },
    listenApprovalResolved: async (handler: (approval: any) => void) => { approvalHandlers.push(handler); return () => {} },
    listenChildRunUpdates: async () => () => {},
    listenRuntimeEvents: async () => () => {},
    listenWorkEvents: async () => () => {},
    listenKernelModelPreviews: async () => () => {},
  },
}))

const { act, cleanup, renderHook, waitFor } = await import('@testing-library/react')
const { useDesktopConversation } = await import('../src/features/conversations/hooks/use-desktop-conversation')
const { approvalInvalidation } = await import('../src/features/conversations/model/approval-labels')

afterEach(() => { cleanup(); approvalHandlers.length = 0 })
afterAll(() => { mock.module('../src/features/conversations/api/desktop-client', () => realDesktopClient) })

const invalidation = {
  code: 'companion.tool_not_available',
  message: '联系人已撤销对工具 send_message 的授权；重新授权后请重新发起，旧的批准不会恢复',
  at: 1700000001000,
}
const approval = (patch: Record<string, unknown> = {}) => ({
  id: 'approval-1', toolCallId: 'call-1', runId: 'run', conversationId: 'chat', toolName: 'call_mcp_tool',
  status: 'pending', requestedAction: '发送消息', request: {}, decision: null,
  requestedAt: 1700000000000, resolvedAt: null, ...patch,
})

const tick = () => new Promise((resolve) => setTimeout(resolve, 5))

describe('approval 事件乱序不回退等待指示与失效原因', () => {
  test('失效结算后迟到的无屏障 pending 事件不重新点亮指示，合法 pending 不受影响', async () => {
    const { result } = renderHook(() => useDesktopConversation())
    await act(async () => { await result.current.openConversation('chat') })
    await waitFor(() => expect(approvalHandlers.length).toBeGreaterThanOrEqual(2))
    await act(async () => { await tick() })
    const emit = async (record: any) => { await act(async () => { approvalHandlers.forEach((handler) => handler(record)); await tick() }) }

    // 1) 合法 pending 请求点亮「等你处理」。
    const requested = approval()
    await emit(requested)
    expect(result.current.runIndicators.get('chat')).toBe('approval')
    expect(result.current.detail?.approvals.map((item: any) => item.id)).toEqual(['approval-1'])

    // 2) 失效结算事件：指示清除，详情保留持久原因。
    const settled = approval({ status: 'approved', resolvedAt: 1700000001000,
      decision: { approved: true, scope: 'once', category: 'tool_execution', invalidation } })
    await emit(settled)
    expect(result.current.runIndicators.get('chat')).not.toBe('approval')
    const record = result.current.detail?.approvals.find((item: any) => item.id === 'approval-1') as any
    expect(record.status).toBe('approved')
    expect(approvalInvalidation(record.decision)?.code).toBe('companion.tool_not_available')

    // 3) 迟到且不带屏障的 pending 重放：不得恢复等待指示，也不得抹掉原因。
    await emit(structuredClone(requested))
    expect(result.current.runIndicators.get('chat')).not.toBe('approval')
    const merged = result.current.detail?.approvals.find((item: any) => item.id === 'approval-1') as any
    expect(merged.status).toBe('approved')
    expect(approvalInvalidation(merged.decision)?.message).toContain('撤销')

    // 4) 另一条合法 pending 批准仍正常点亮指示。
    await emit(approval({ id: 'approval-2', toolCallId: 'call-2' }))
    expect(result.current.runIndicators.get('chat')).toBe('approval')

    // 5) 全量重读（持久事实为准）后原因仍在，失效记录不回到等待队列。
    persisted.approvals = [settled]
    await act(async () => { await result.current.openConversation('chat'); await tick() })
    const reloaded = result.current.detail?.approvals.find((item: any) => item.id === 'approval-1') as any
    expect(approvalInvalidation(reloaded.decision)?.code).toBe('companion.tool_not_available')
  })
})