import { afterAll, afterEach, expect, mock, test } from 'bun:test'
import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import type { ArtifactRecord, ConversationDetail } from '../src/features/conversations/model/types'
import type { ContextBudgetDto } from '../src/features/chat/components/context-budget-presentation'

const actualClient = { ...await import('../src/features/conversations/api/desktop-client') }
mock.module('../src/features/conversations/api/desktop-client', () => ({ ...actualClient, desktopRuntimeAvailable: true }))
const { ConversationSummary } = await import('../src/features/chat/right-panel/conversation-summary')
const { useContextBudget } = await import('../src/features/chat/hooks/use-context-budget')
const visibility = Object.getOwnPropertyDescriptor(document, 'visibilityState')

afterEach(() => {
  cleanup()
  clearMocks()
  if (visibility) Object.defineProperty(document, 'visibilityState', visibility)
  else Reflect.deleteProperty(document, 'visibilityState')
})
afterAll(() => { mock.module('../src/features/conversations/api/desktop-client', () => actualClient) })

const budget: ContextBudgetDto = { modelWindowTokens: 100000, historyTokens: 50000, toolsTokens: 10000, systemTokens: 5000, pendingAppendTokens: 0, outputReserveTokens: 10000, safetyMarginTokens: 5000, availableTokens: 20000, pressureRatio: .8, estimateSource: 'heuristic', updatedAt: 1 }
const detail = { conversation: { id: 'budget-summary', projectRoot: null }, lastRun: { status: 'completed' }, tasks: [], artifacts: [], toolCalls: [], childRuns: [], runtimeEvents: [], knowledgeBindings: [] } as unknown as ConversationDetail

test('收起模型行在 80% 显示警告，90% 显示错误颜色', async () => {
  let pressure = .8
  mockIPC(command => ({ ok: true, data: command === 'conversation_command_tasks' ? [] : { runId: null, jobs: [], contextBudget: { ...budget, pressureRatio: pressure } } }))
  const view = render(<ConversationSummary detail={detail} modelName="模型 A" onMode={() => {}} />)
  fireEvent.click(view.getByRole('button', { name: /模型 A/, expanded: true }))
  await waitFor(() => expect(view.getByText('80%').className).toBe('is-warning'))
  pressure = .9
  view.rerender(<ConversationSummary key="next" detail={{ ...detail, conversation: { ...detail.conversation, id: 'budget-next' } }} modelName="模型 B" onMode={() => {}} />)
  fireEvent.click(view.getByRole('button', { name: /模型 B/, expanded: true }))
  await waitFor(() => expect(view.getByText('90%').className).toBe('is-error'))
})

test('Pi 会话用量在没有旧预算的情况下显示方格和可用量', async () => {
  mockIPC(command => ({ ok: true, data: command === 'conversation_command_tasks' ? [] : { runId: 'pi-run', jobs: [], contextBudget: null, contextWindow: { source: 'session', usedTokens: 32768, limitTokens: 65536 } } }))
  const view = render(<ConversationSummary detail={detail} modelName="模型 A" onMode={() => {}} />)
  const meter = await view.findByRole('meter', { name: '上下文占用' })
  expect(meter.getAttribute('aria-valuenow')).toBe('50')
  expect(meter.querySelectorAll('.is-history')).toHaveLength(72)
  expect(meter.querySelectorAll('.is-free')).toHaveLength(72)
  expect(view.getByText('按当前会话估算')).toBeDefined()
})

test('生成文件展开全部时也包含未加载的历史轮次', async () => {
  const files = Array.from({ length: 12 }, (_, i) => ({ id: String(i), displayName: `历史文件 ${i}.md`, createdAt: i, updatedAt: i }))
  mockIPC(command => {
    if (command === 'conversation_summary_files') return { ok: true, data: files }
    if (command === 'conversation_command_tasks') return { ok: true, data: [] }
    return { ok: true, data: { runId: null, jobs: [], contextBudget: null } }
  })
  const view = render(<ConversationSummary detail={{ ...detail, hasEarlierMessages: true, artifacts: files.slice(-2) as ArtifactRecord[] }} modelName="模型 A" onMode={() => {}} />)
  fireEvent.click(await view.findByRole('button', { name: '查看更多' }))
  expect(view.queryByText('历史文件 0.md')).toBeNull()
  fireEvent.click(view.getByRole('button', { name: '查看更多' }))
  expect(view.getByText('历史文件 0.md')).toBeDefined()
  expect(view.getByText('12 个')).toBeDefined()
})

function BudgetProbe({ enabled, running }: { enabled: boolean; running: boolean }) {
  const { budget: value } = useContextBudget('poll-test', running, enabled)
  return <span>{value?.updatedAt ?? '等待'}</span>
}

test('共享 hook 运行中轮询，隐藏页面和禁用后停止，重新可见时立即刷新', async () => {
  let calls = 0
  mockIPC(command => {
    expect(command).toBe('kernel_reliability_snapshot')
    return { ok: true, data: { runId: null, jobs: [], contextBudget: { ...budget, updatedAt: ++calls } } }
  })
  const view = render(<BudgetProbe enabled running />)
  await waitFor(() => expect(calls).toBe(1))
  await waitFor(() => expect(calls).toBe(2), { timeout: 2500 })
  await act(async () => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' })
    document.dispatchEvent(new Event('visibilitychange'))
    await new Promise(resolve => setTimeout(resolve, 1600))
  })
  expect(calls).toBe(2)
  await act(async () => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' })
    document.dispatchEvent(new Event('visibilitychange'))
  })
  await waitFor(() => expect(calls).toBe(3))
  view.rerender(<BudgetProbe enabled={false} running />)
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 1600)) })
  expect(calls).toBe(3)
})
