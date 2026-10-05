import { describe, expect, test } from 'bun:test'
import {
  allowedApprovalDecisions,
  repairOverrideApprovalDetails,
  resolveAllowedApprovalDecision,
} from '../src/features/conversations/model/approval-decision-policy'
import type { ApprovalRecord, ApprovalRequest } from '../src/features/conversations/model/types'

function overrideRequest(overrides: Partial<ApprovalRequest> = {}): ApprovalRequest {
  return {
    category: 'task_repair_budget_override',
    availableDecisions: ['allow_once', 'deny'],
    arguments: {
      rootCause: '已有修复没有覆盖并发写入路径',
      findingIds: ['finding-1', 'finding-2'],
      escalationReason: '普通返工预算已经用完',
    },
    ...overrides,
  }
}

function approval(request: ApprovalRequest): Pick<ApprovalRecord, 'id' | 'request'> {
  return { id: 'approval-1', request }
}

describe('approval decision policy', () => {
  test('Kernel replacement binding only grants one dispatch even without top-level decisions', async () => {
    const binding = { purpose: '整文件替换', requestDigest: 'req', targetIdentity: 'a.md', baselineVersion: 'missing', candidateDigest: 'content', availableDecisions: ['allow_once', 'deny'] }
    const request = { wholeFileReplacement: binding }
    expect(allowedApprovalDecisions(request)).toEqual(['deny', 'allow_once'])
    expect(allowedApprovalDecisions({ ...request, availableDecisions: ['deny', 'allow_conversation', 'allow_once'] })).toEqual(['deny', 'allow_once'])
    expect(allowedApprovalDecisions({ wholeFileReplacement: { ...binding, availableDecisions: ['deny'] } })).toEqual(['deny'])
    expect(allowedApprovalDecisions({ wholeFileReplacement: {} })).toEqual(['deny'])
    let submitted = false
    expect(await resolveAllowedApprovalDecision(approval(request), 'allow_conversation', () => { submitted = true })).toBe(false)
    expect(submitted).toBe(false)
  })
  test('offers the conversation decision for a new file the Host declared reusable', async () => {
    // 新建（目标不存在）不是整文件替换：没有已有内容可被破坏，因此按 Host 的
    // 顶层声明给出会话级授权；后端确实接受它（scope 可精确命名）。
    const create = {
      authority: 'kernel',
      input: { path: 'notes.md', content: 'hello\n' },
      availableDecisions: ['allow_once', 'allow_conversation', 'deny'],
    }
    expect(allowedApprovalDecisions(create)).toEqual([
      'deny',
      'allow_once',
      'allow_conversation',
    ])
    const sent: Array<[string, string]> = []
    expect(await resolveAllowedApprovalDecision(approval(create), 'allow_conversation', (id, decision) => {
      sent.push([id, decision])
      return true
    })).toBeTrue()
    expect(sent).toEqual([['approval-1', 'allow_conversation']])
  })

  test('keeps a real replacement one-dispatch even when the Host declares a conversation decision', async () => {
    // 已有内容的整份覆盖：一次性替换凭据，绝不升级为会话级授权。
    const replacement = {
      authority: 'kernel',
      availableDecisions: ['allow_once', 'allow_conversation', 'deny'],
      wholeFileReplacement: {
        purpose: '整文件替换',
        requestDigest: 'req',
        targetIdentity: 'a.md',
        baselineVersion: 'sha256:existing',
        candidateDigest: 'content',
        availableDecisions: ['allow_once', 'deny'],
      },
    }
    expect(allowedApprovalDecisions(replacement)).toEqual(['deny', 'allow_once'])
    const sent: Array<[string, string]> = []
    expect(await resolveAllowedApprovalDecision(approval(replacement), 'allow_conversation', (id, decision) => {
      sent.push([id, decision])
      return true
    })).toBeFalse()
    expect(sent).toEqual([])
  })

  test('never widens a layer that withheld the conversation decision for a new file', () => {
    // 新建也不放宽：嵌套声明里没有的选项不会被补回来。
    const create = {
      authority: 'kernel',
      availableDecisions: ['allow_once', 'allow_conversation', 'deny'],
      wholeFileReplacement: {
        purpose: '整文件替换',
        requestDigest: 'req',
        targetIdentity: 'a.md',
        baselineVersion: 'missing',
        candidateDigest: 'content',
        availableDecisions: ['allow_once', 'deny'],
      },
    }
    expect(allowedApprovalDecisions(create)).toEqual(['deny', 'allow_once'])
  })

  test('shows only allow once and deny for a repair budget override', () => {
    expect(allowedApprovalDecisions(overrideRequest())).toEqual(['deny', 'allow_once'])
    expect(allowedApprovalDecisions(overrideRequest({
      availableDecisions: ['allow_once', 'deny', 'allow_conversation'],
    }))).toEqual(['deny', 'allow_once'])
    expect(repairOverrideApprovalDetails(overrideRequest())).toEqual({
      rootCause: '已有修复没有覆盖并发写入路径',
      findingIds: ['finding-1', 'finding-2'],
    })
  })

  test('keeps the legacy three choices for an ordinary approval with no declaration', () => {
    expect(allowedApprovalDecisions({})).toEqual([
      'deny',
      'allow_once',
      'allow_conversation',
    ])
    expect(allowedApprovalDecisions({ category: 'tool_execution' })).toEqual([
      'deny',
      'allow_once',
      'allow_conversation',
    ])
  })

  test('Kernel choices remain one-shot until the Host supplies its scoped decision list', () => {
    expect(allowedApprovalDecisions({ authority: 'kernel' })).toEqual(['deny', 'allow_once'])
    expect(allowedApprovalDecisions({ authority: 'kernel', availableDecisions: ['allow_once', 'allow_conversation', 'deny'] }))
      .toEqual(['deny', 'allow_once', 'allow_conversation'])
  })

  test('fails closed for empty, duplicate, or unknown decision declarations', () => {
    expect(allowedApprovalDecisions({ availableDecisions: [] })).toEqual(['deny'])
    expect(allowedApprovalDecisions({ availableDecisions: ['allow_once', 'allow_once'] })).toEqual(['deny'])
    expect(allowedApprovalDecisions({ availableDecisions: ['allow_once', 'allow_forever'] })).toEqual(['deny'])
    expect(allowedApprovalDecisions({ availableDecisions: 'allow_once' })).toEqual(['deny'])
  })

  test('does not grant choices to an unknown approval category', () => {
    expect(allowedApprovalDecisions({
      category: 'future_super_grant',
      availableDecisions: ['deny', 'allow_once', 'allow_conversation'],
    })).toEqual(['deny'])
  })

  test('does not add allow once when the Host did not declare it', () => {
    expect(allowedApprovalDecisions({
      category: 'tool_execution',
      availableDecisions: ['deny'],
    })).toEqual(['deny'])
  })

  test('keeps only deny when repair override context is malformed', () => {
    expect(allowedApprovalDecisions(overrideRequest({ arguments: { rootCause: '', findingIds: [] } }))).toEqual(['deny'])
  })

  test('does not send a decision that the Host did not allow', async () => {
    const sent: Array<[string, string]> = []
    const resolver = async (approvalId: string, decision: string) => {
      sent.push([approvalId, decision])
      return true
    }

    expect(await resolveAllowedApprovalDecision(
      approval(overrideRequest()),
      'allow_conversation',
      resolver,
    )).toBeFalse()
    expect(sent).toEqual([])

    expect(await resolveAllowedApprovalDecision(
      approval(overrideRequest()),
      'allow_once',
      resolver,
    )).toBeTrue()
    expect(sent).toEqual([['approval-1', 'allow_once']])
  })
})
