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
