import test from 'node:test'
import assert from 'node:assert/strict'
import { parsePlannerOutput, plannerHandoff, shouldUsePlanner } from '../src/planner-runtime.mjs'

test('uses the planner only for project-scoped multi-step execution requests', () => {
  assert.equal(shouldUsePlanner('请分析项目，然后修改代码并运行测试', { hasProject: true }), true)
  assert.equal(shouldUsePlanner('轨道吊起升怎么赋值', { hasProject: true }), false)
  assert.equal(shouldUsePlanner('请解释这段 Markdown', { hasProject: true }), false)
  assert.equal(shouldUsePlanner('请修改代码并运行测试', { hasProject: false }), false)
  assert.equal(shouldUsePlanner('请修改代码并运行测试', { hasProject: true, approvalDemo: true }), false)
  assert.equal(shouldUsePlanner('请修改代码并运行测试', { hasProject: true, apiType: 'faux' }), false)
  assert.equal(shouldUsePlanner('请把下面这个独立子任务委派给合适的子 Agent，并在完成后汇总、验证它的结果：\n\n优化下前面生成的代码', { hasProject: true }), false)
  assert.equal(shouldUsePlanner('/plan 优化前面生成的代码', { hasProject: true }), true)
  assert.equal(shouldUsePlanner('/计划 优化前面生成的代码', { hasProject: true }), true)
  assert.equal(shouldUsePlanner('/plan 优化前面生成的代码', { hasProject: false }), false)
  assert.equal(shouldUsePlanner('test planner', { hasProject: true, apiType: 'faux', force: true }), true)
})

test('normalizes fenced planner JSON and bounds plan size', () => {
  const parsed = parsePlannerOutput('```json\n{"summary":"修复运行时","steps":["检查事件","修改映射","运行测试"],"risks":["终态重复"],"needsGoal":true}\n```')
  assert.deepEqual(parsed, {
    summary: '修复运行时',
    steps: ['检查事件', '修改映射', '运行测试'],
    risks: ['终态重复'],
    needsGoal: true,
  })
})

test('marks planner handoff as advisory instead of Host authority', () => {
  const handoff = plannerHandoff({ summary: 'Plan', steps: ['Inspect'], risks: [], needsGoal: false })
  assert.match(handoff, /"authority": "advisory"/)
  assert.match(handoff, /Fox Host remains authoritative/)
})
