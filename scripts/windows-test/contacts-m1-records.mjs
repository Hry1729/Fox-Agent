import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

export async function checkContactRecords({ invoke, db, check, expert, builtin, directory }) {
  const owner = { companionId: expert.id }
  const projectRoot = join(directory, 'records-project')
  await mkdir(projectRoot)
  const { agent_id: agentId } = db.prepare('SELECT agent_id FROM conversations WHERE id=?').get(builtin.conversationId)
  const { projectId } = await invoke('conversation_create', {
    request: { agentId, expertId: expert.templateAgentId, title: '验收项目', projectRoot },
  })
  assert.ok(projectId)

  await check('A22 save an over-budget draft and convert it to a topic', async () => {
    await invoke('companion_note_save_user', { ...owner, content: '价'.repeat(3900) })
    const content = '📦'.repeat(101)
    const overflow = await invoke('companion_note_save_user', { ...owner, content })
    assert.equal(overflow.overBy, 1)
    const notes = await invoke('companion_notes_get', owner)
    assert.equal(notes.usedChars, 3900)
    assert.equal(notes.draft, content)
    const { topicId } = await invoke('companion_note_draft_to_topic', { ...owner, title: '超额草稿' })
    const topic = await invoke('companion_topic_read', { ...owner, topicId })
    assert.equal(topic.content, content)
    assert.equal((await invoke('companion_notes_get', owner)).draft, null)
    return { mainChars: notes.usedChars, draftChars: [...topic.content].length }
  })

  await check('A23 reject stale topic edits and remove search fragments on deletion', async () => {
    const title = '区域报价'
    const created = await invoke('companion_topic_save', { ...owner, title, content: '报价编号 XQ-7731' })
    const args = { ...owner, title, topicId: created.topicId, expectedSha: created.contentSha256 }
    const edited = await invoke('companion_topic_save', { ...args, content: '报价编号 XQ-7732' })
    const stale = await invoke('companion_topic_save', { ...args, content: '另一个编辑器的草稿' })
    assert.equal(stale.status, 'conflict')
    assert.equal(stale.draft, '另一个编辑器的草稿')
    assert.equal(stale.current.content, '报价编号 XQ-7732')
    const chunks = db.prepare('SELECT COUNT(*) AS n FROM companion_note_chunks WHERE topic_id=?')
    const beforeDelete = chunks.get(created.topicId).n
    assert.ok(beforeDelete > 0)
    await invoke('companion_topic_delete', { ...owner, topicId: created.topicId, expectedSha: edited.contentSha256 })
    assert.equal(chunks.get(created.topicId).n, 0)
    assert.ok(!(await invoke('companion_notes_get', owner)).topics.some(t => t.id === created.topicId))
    return { fragmentsBefore: beforeDelete, fragmentsAfter: 0 }
  })

  await check('A04/A09 archive, restore, revoke and re-add', async () => {
    const grant = { ...owner, projectId }
    await invoke('companion_grant_project', grant)
    const originalGrants = await invoke('companion_grants_list', owner)
    assert.equal(originalGrants.length, 1)
    const archived = await invoke('companion_archive', { request: owner })
    assert.ok(archived.archivedAt)
    const restored = await invoke('companion_restore', { request: owner })
    assert.equal(restored.archivedAt, null)
    assert.equal(restored.conversationId, expert.conversationId)
    assert.deepEqual(await invoke('companion_grants_list', owner), originalGrants)
    assert.equal((await invoke('companion_notes_get', owner)).usedChars, 3900)
    await invoke('companion_revoke_project', grant)
    assert.deepEqual(await invoke('companion_grants_list', owner), [])
    await invoke('companion_grant_project', grant)
    const { planHash, plan } = await invoke('companion_delete_preview', owner)
    assert.equal(plan.grantedProjects.length, 1)
    await invoke('companion_delete', { ...owner, planHash })
    const next = await invoke('companion_add_from_expert', { request: { expertId: expert.templateAgentId } })
    assert.notEqual(next.id, expert.id)
    assert.notEqual(next.conversationId, expert.conversationId)
    assert.notEqual(next.workspaceDir, expert.workspaceDir)
    const nextOwner = { companionId: next.id }
    assert.deepEqual(await invoke('companion_grants_list', nextOwner), [])
    const empty = await invoke('companion_notes_get', nextOwner)
    assert.equal(empty.usedChars, 0)
    assert.equal(empty.topics.length, 0)
    return { restoredGrant: originalGrants[0].id, newContactStartsEmpty: true }
  })
}
