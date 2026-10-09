import test from 'node:test'
import assert from 'node:assert/strict'

test('placement gives named Host targets priority over default placement and exposes unknown recognition', () => {
  assert.match(FOX_RUNTIME_INSTRUCTIONS, /bare filename at the project root/u)
  assert.match(FOX_RUNTIME_INSTRUCTIONS, /Host-frozen target/u)
  assert.match(FOX_RUNTIME_INSTRUCTIONS, /unverified/u)
  assert.doesNotMatch(FOX_RUNTIME_INSTRUCTIONS, /a CSV the user asked for is a deliverable and belongs under deliverableRoot/u)
})
import {
  APPROVAL_DEMO_TURN_INSTRUCTIONS,
  FOX_LANGUAGE_CONTRACT,
  FOX_RUNTIME_INSTRUCTIONS,
  isApprovalDemoFollowup,
  isApprovalDemoRequest,
  replaceLatestAssistantText,
  runtimeSystemPrompt,
} from '../src/runtime-instructions.mjs'

test('requires Simplified Chinese for user-facing text and preserves machine text', () => {
  // One fragment, embedded verbatim, so the rule cannot drift between composers.
  assert.ok(FOX_RUNTIME_INSTRUCTIONS.startsWith(FOX_LANGUAGE_CONTRACT))
  assert.match(FOX_LANGUAGE_CONTRACT, /Simplified Chinese \(简体中文\)/)
  for (const channel of ['progress notes', 'action descriptions', 'failure', 'stage and phase summaries', 'final response']) {
    assert.ok(FOX_LANGUAGE_CONTRACT.includes(channel), `the rule must name the ${channel} channel`)
  }
  // User instruction outranks the default.
  assert.match(FOX_LANGUAGE_CONTRACT, /explicit user language choice outranks this default/)
  // Code, commands, paths, identifiers and necessary quotations keep their form.
  assert.match(FOX_LANGUAGE_CONTRACT, /code, commands, shell flags, paths, file names, URLs, identifiers/)
  assert.match(FOX_LANGUAGE_CONTRACT, /A necessary original quotation stays in its own language/)
  // Private reasoning stays a non-user-facing channel.
  assert.match(FOX_LANGUAGE_CONTRACT, /Private reasoning and internal planning are not user-facing/)
  assert.equal(runtimeSystemPrompt('Base prompt').includes(FOX_LANGUAGE_CONTRACT), true)
  assert.equal(runtimeSystemPrompt('Base prompt', { approvalDemo: true }).includes(FOX_LANGUAGE_CONTRACT), true)
})

test('publishes protected tools and truthful approval rules in the system prompt', () => {
  const prompt = runtimeSystemPrompt('Base prompt')
  for (const tool of ['write_file', 'edit_file', 'run_command', 'call_mcp_tool']) {
    assert.match(prompt, new RegExp(`\\b${tool}\\b`))
  }
  assert.match(FOX_RUNTIME_INSTRUCTIONS, /real protected tool call/i)
  assert.match(FOX_RUNTIME_INSTRUCTIONS, /Do not simulate commands, approvals, denials, timeouts, or tool results/i)
  assert.match(FOX_RUNTIME_INSTRUCTIONS, /sequentially/i)
  assert.match(FOX_RUNTIME_INSTRUCTIONS, /powershell -NoProfile -Command/)
})

test('requires Host-validated goal completion and distinguishes A0 from A1 acceptance', () => {
  assert.match(FOX_RUNTIME_INSTRUCTIONS, /work_snapshot_get/)
  assert.match(FOX_RUNTIME_INSTRUCTIONS, /goal_complete/)
  assert.match(FOX_RUNTIME_INSTRUCTIONS, /Host remains authoritative/)
  assert.match(FOX_RUNTIME_INSTRUCTIONS, /Independent review and acceptance are A1 capabilities/)
})

test('keeps assistant presentation plain and structurally aligned', () => {
  assert.match(FOX_RUNTIME_INSTRUCTIONS, /Do not use emoji/)
  assert.match(FOX_RUNTIME_INSTRUCTIONS, /left-aligned/)
  assert.match(FOX_RUNTIME_INSTRUCTIONS, /real Markdown bullets or numbers/)
})

test('adds a turn-specific tool-call requirement for approval demonstrations', () => {
  const prompt = runtimeSystemPrompt('Base prompt', { approvalDemo: true })
  assert.ok(prompt.includes(APPROVAL_DEMO_TURN_INSTRUCTIONS))
  assert.match(prompt, /must make at least one real call/i)
})

test('recognizes explicit and contextual approval demonstration requests', () => {
  assert.equal(isApprovalDemoRequest('请触发几个审批弹窗让我看看'), true)
  assert.equal(isApprovalDemoRequest('再触发两个弹窗测试一下'), true)
  assert.equal(isApprovalDemoRequest('Show me a permission approval dialog'), true)
  assert.equal(isApprovalDemoRequest('解释一下审批机制'), false)
  assert.equal(isApprovalDemoRequest('实现一个普通设置弹窗'), false)
})

test('recognizes only a short follow-up to a previous approval demonstration', () => {
  assert.equal(isApprovalDemoFollowup('再来两个', '请触发审批弹窗让我看看'), true)
  assert.equal(isApprovalDemoFollowup('谢谢', '请触发审批弹窗让我看看'), false)
  assert.equal(isApprovalDemoFollowup('继续修改项目代码', '请触发审批弹窗让我看看'), false)
})

test('replaces a fabricated assistant result before it is persisted into history', () => {
  const messages = [
    { role: 'user', content: '触发审批弹窗' },
    { role: 'assistant', content: [{ type: 'text', text: '三个动作都被拦截。' }], model: 'test' },
  ]
  const replaced = replaceLatestAssistantText(messages, '没有实际调用任何工具。')
  assert.deepEqual(replaced[1].content, [{ type: 'text', text: '没有实际调用任何工具。' }])
  assert.equal(replaced[1].model, 'test')
  assert.notEqual(replaced, messages)
})
