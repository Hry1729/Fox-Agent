import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'

test('extensions API consumers unwrap Yuxi success/data payloads', async () => {
  const util = await readFile(new URL('../src/utils/apiData.ts', import.meta.url), 'utf8')
  assert.match(util, /unwrapApiData/)
  assert.match(util, /unwrapList/)

  const mcp = await readFile(
    new URL('../src/components/extensions/mcp/McpCardList.vue', import.meta.url),
    'utf8'
  )
  assert.match(mcp, /unwrapList/)

  const skills = await readFile(
    new URL('../src/components/extensions/skills/SkillCardList.vue', import.meta.url),
    'utf8'
  )
  assert.match(skills, /listSkills/)
  assert.match(skills, /unwrapList/)
  assert.doesNotMatch(skills, /内置 Skills/)
  assert.doesNotMatch(skills, /name: '内置'/)

  const knowledge = await readFile(
    new URL('../src/store/modules/knowledge.ts', import.meta.url),
    'utf8'
  )
  assert.match(knowledge, /kb_types/)

  const create = await readFile(
    new URL('../src/components/extensions/knowledge/KnowledgeCreateDialog.vue', import.meta.url),
    'utf8'
  )
  assert.match(create, /database_name/)
  assert.match(create, /create_params/)
})
