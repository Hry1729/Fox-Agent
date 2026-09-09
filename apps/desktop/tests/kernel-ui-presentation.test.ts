import { expect, test } from 'bun:test'
import { approvalPresentation } from '../src/features/conversations/model/approval-presentation'
import { assistantDisplayContent, latestRunAssistantId } from '../src/features/chat/runtime-timeline-performance'
import type { ApprovalRequest, ConversationMessage } from '../src/features/conversations/model/types'

const present = (request: ApprovalRequest) => approvalPresentation({ request, toolName: 'write_file', requestedAction: 'write_file' })
const message = (id: string, content: string, runId = 'run'): ConversationMessage => ({ id, content, runId, conversationId: 'c', role: 'assistant', kind: 'text', status: 'completed', ordinal: 1, createdAt: 1, updatedAt: 1 })

test('Kernel nested write input exposes exact path and content including empty files', () => {
  expect(present({ authority: 'kernel', input: { path: 'approval-proof.txt', content: 'FOX-KERNEL-APPROVAL-20260909' } })).toMatchObject({ target: 'approval-proof.txt', content: 'FOX-KERNEL-APPROVAL-20260909' })
  expect(present({ input: { path: '中文.txt', content: '' } }).content).toBe('')
})
test('legacy fields take precedence and malformed nested input is safe', () => {
  expect(present({ title: '标题', target: 'legacy', summary: '摘要', command: 'old', diff: 'diff', input: { path: 'new', command: 'new' } })).toMatchObject({ title: '标题', target: 'legacy', summary: '摘要', command: 'old', diff: 'diff' })
  for (const input of [null, [], 42, { path: {}, content: false }]) expect(present({ input }).target).toBe('write_file')
  expect(present({ input: { command: 'pwd', cwd: 'project' } })).toMatchObject({ command: 'pwd', target: 'project' })
})
test('multi-round Kernel messages retain their own content, only the latest receives streaming text', () => {
  const messages = [message('tool-proposal', '\n\n'), message('final', 'answer'), message('other', 'other answer', 'other')]
  const target = latestRunAssistantId(messages, 'run')
  expect(target).toBe('final')
  expect(messages.map(m => assistantDisplayContent(m, target, 'live answer'))).toEqual(['\n\n', 'live answer', 'other answer'])
  expect(assistantDisplayContent(messages[1], target, '')).toBe('answer')
  expect(latestRunAssistantId(messages)).toBeUndefined()
  expect(latestRunAssistantId([], 'run')).toBeUndefined()
})
test('legacy single assistant and synthetic streaming placeholder still receive the stream', () => {
  const m = message('assistant-run', '')
  expect(assistantDisplayContent(m, latestRunAssistantId([m], 'run'), 'stream')).toBe('stream')
})
