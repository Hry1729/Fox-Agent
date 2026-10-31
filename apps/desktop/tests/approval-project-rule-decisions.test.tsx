import { describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { allowedApprovalDecisions } from '../src/features/conversations/model/approval-decision-policy'
import { RuntimeApprovalPrompt } from '../src/features/chat/components/RuntimeApprovalPrompt'
import type { ApprovalRecord } from '../src/features/conversations/model/types'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

describe('allow_project_rule is a known non-settling declaration', () => {
  test('keeps deny and allow once when the host also offers a project rule', () => {
    expect(allowedApprovalDecisions({
      availableDecisions: ['allow_once', 'allow_project_rule', 'deny'],
    })).toEqual(['deny', 'allow_once'])
  })

  test('still fails closed on an unknown decision', () => {
    expect(allowedApprovalDecisions({
      availableDecisions: ['allow_once', 'bogus', 'deny'],
    })).toEqual(['deny'])
  })

  test('still fails closed when allow_project_rule is repeated', () => {
    expect(allowedApprovalDecisions({
      availableDecisions: ['allow_once', 'allow_project_rule', 'allow_project_rule', 'deny'],
    })).toEqual(['deny'])
  })
})

function commandCard(shape: 'pi' | 'pi-native'): ApprovalRecord {
  const command = 'cd "D:\\fox-test\\测试项目" && dir 2>&1; echo "EXIT_CODE: $?"'
  return {
    id: shape === 'pi' ? 'pi-approval-1' : 'native-approval-1',
    toolCallId: 'call-1',
    runId: 'run-1',
    conversationId: 'conversation-1',
    toolName: 'run_command',
    status: 'pending',
    requestedAction: '执行命令',
    decision: null,
    requestedAt: 0,
    resolvedAt: null,
    request: {
      command,
      cwd: 'D:/fox-test/测试项目',
      availableDecisions: ['allow_once', 'allow_project_rule', 'deny'],
      projectRule: { eligible: true, tool: 'run_command', suggestedPrefix: 'dir' },
      policy: { risk: 'low', findings: ['exit_marker_wrapper'] },
      // 外部 Pi 的 hostExecution 是 null；PiNative 带 Host 执行说明。
      hostExecution: shape === 'pi' ? null : { isolation: 'none', note: '在 Host 上以当前用户身份执行，不受沙箱限制' },
    },
  }
}

async function expectThreeActions(shape: 'pi' | 'pi-native') {
  const calls: unknown[] = []
  const approval = commandCard(shape)
  const host = document.createElement('div')
  const root = createRoot(host)
  try {
    await act(async () => root.render(
      <RuntimeApprovalPrompt
        approval={approval}
        onResolve={async (id, decision) => {
          calls.push([id, decision])
          return true
        }}
      />,
    ))
    const buttons = [...host.querySelectorAll('button')]
    const deny = buttons.find(button => button.textContent === '拒绝')
    const once = buttons.find(button => button.textContent === '只允许这一次')
    const project = buttons.find(button => button.textContent?.includes('本项目总是允许'))
    expect(deny).toBeTruthy()
    expect(once).toBeTruthy()
    expect(project).toBeTruthy()
    await act(async () => once!.click())
    expect(calls).toEqual([[approval.id, 'allow_once']])
  } finally {
    await act(async () => root.unmount())
  }
}

test('an external Pi command card can allow once and save a project rule', async () => {
  await expectThreeActions('pi')
})

test('a PiNative command card can allow once and save a project rule', async () => {
  await expectThreeActions('pi-native')
})
