import test from 'node:test'
import assert from 'node:assert/strict'
import { RUNTIME_TOOL_CATALOG } from '../src/runtime-contract.mjs'
import { selectCompanionTools } from '../src/companion-tools.mjs'
import { diagnoseToolsForAgentContext } from '../src/expert-package.mjs'
import { composeFoxPrompt } from '../src/prompt-composer.mjs'
import { companionMemoryPrompt } from '../src/companion-memory-prompt.mjs'

const noteNames = ['memory_write', 'note_organize', 'note_search', 'note_read']
const catalog = RUNTIME_TOOL_CATALOG
const names = (context, expert = null) => selectCompanionTools(
  catalog, diagnoseToolsForAgentContext(catalog, null, expert), context,
).map(tool => tool.name)

test('ordinary chats retain their previous catalog and projects only gain granted read tools', () => {
  assert.deepEqual(names(null), catalog.map(t => t.name).filter(name => !noteNames.includes(name)))
  assert.deepEqual(names({ role: 'reader' }).filter(name => noteNames.includes(name)), ['note_search', 'note_read'])
})

test('a companion can use notes without gaining project tools excluded by its expert', () => {
  const allowedTools = ['read', 'grep', 'write_file', ...noteNames]
  const expert = { packageManifest: { allowedTools: ['read', 'git_read', 'sqlite_read'] } }
  assert.deepEqual(names({ role: 'owner', allowedTools }, expert), ['read', ...noteNames])
})

test('the prompt distinguishes the read-only project view from a private profile', () => {
  const facts = {
    displayName: '财务', duty: '仅在私聊展示的职责', personaOverride: '逐步讲解',
    usedChars: 4, notes: [{ id: 'a', author: 'user', content: '先看结论' }],
    topics: [{ id: 'b', title: '十月报价' }], snippets: [{ topicId: 'b', text: '由用户核对' }],
  }
  const owner = companionMemoryPrompt({ ...facts, role: 'owner' })
  const reader = companionMemoryPrompt({ ...facts, role: 'reader' })
  assert.ok(owner.required.includes(facts.duty) && owner.required.includes('4/4000'))
  assert.ok(owner.required.includes('当前私聊不支持定时任务'))
  assert.match(owner.required, /"author":"user"/)
  assert.match(owner.required, /note_organize/)
  assert.match(owner.required, /本次会话的压缩摘要/)
  assert.doesNotMatch(reader.required, /本次会话的压缩摘要/)
  assert.ok(!reader.required.includes('"author"'))
  assert.ok(reader.required.includes('只读') && reader.required.includes('先看结论') && reader.topics.includes('十月报价'))
  assert.ok(!reader.required.includes(facts.duty) && !reader.required.includes(facts.personaOverride))
  assert.ok(!reader.required.includes('定时任务'))
  assert.equal(companionMemoryPrompt(null), null)
})


test('the actual composer includes every main note or reports an insufficient budget', () => {
  const facts = {
    role: 'owner', displayName: '项目助手', usedChars: 4000, topics: [], snippets: [],
    notes: [{ id: 'head', content: '猫'.repeat(3999) }, { id: 'tail', content: '尾' }],
  }
  const context = { companionContext: facts }
  const result = composeFoxPrompt({ context, budget: { maxPromptChars: 48_000 } })
  assert.ok(result.dynamicTail.includes('猫'.repeat(3999)))
  assert.ok(result.dynamicTail.includes('尾'))
  assert.throws(() => composeFoxPrompt({ context, budget: { maxPromptChars: 2048 } }), /companion.note_context_budget/)
})

test('manual adoption places current expert instructions last in a private session', () => {
  const prompt = '报价先按含税口径核对。'
  const context = {
    expertBinding: { activationSource: 'companion_template_update' },
    expertPackage: { name: '采购专家', systemPrompt: prompt, packageManifest: { prompt, skills: ['采购技能'] } },
    companionContext: { role: 'owner', displayName: '采购小狐', usedChars: 0, notes: [], topics: [], snippets: [] },
  }
  const current = composeFoxPrompt({ context }).dynamicTail
  assert.ok(current.includes('采购技能'))
  assert.ok(current.indexOf('用户已在联系人资料页采用新版专家模板') > current.indexOf('Current-turn diagnostics'))
  assert.equal(current.split(prompt).length, 2)
  const distinct = composeFoxPrompt({ context: { ...context,
    expertPackage: { ...context.expertPackage, packageManifest: { prompt: '保存已审批的报价依据。' } },
  } }).dynamicTail
  assert.ok(distinct.includes('保存已审批的报价依据。'))
  assert.ok(distinct.includes(prompt))
  const original = composeFoxPrompt({ context: { ...context, expertBinding: { activationSource: 'companion_create' } } }).dynamicTail
  assert.doesNotMatch(original, /已在联系人资料页采用新版专家模板/)
  assert.ok(original.includes(prompt))
})

test('an expert only explains unavailable notes when this conversation cannot read them', () => {
  const expertBinding = { expertId: 'procurement', activationSource: 'user' }
  const prompt = context => composeFoxPrompt({ context }).dynamicTail
  const unavailable = '本轮无法读取联系人笔记'
  assert.ok(prompt({ expertBinding }).includes(unavailable))
  assert.ok(!prompt({}).includes(unavailable))
  for (const role of ['reader', 'owner']) {
    const companionContext = { role, notes: [], topics: [], snippets: [] }
    assert.ok(!prompt({ expertBinding, companionContext }).includes(unavailable))
  }
})
