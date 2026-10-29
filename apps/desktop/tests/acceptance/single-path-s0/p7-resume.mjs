// P7 re-submitting a toolResult vs branching back past the orphaned assistant message.
// Run: node services/agent-runtime/.probe-s0/p7-resume.mjs   (requires p7-artifact.mjs first)
import { readFileSync, existsSync, copyFileSync } from 'node:fs'
import { SessionManager } from '@earendil-works/pi-coding-agent'
import { fauxAssistantMessage, Type } from '@earendil-works/pi-ai'
import { transformMessages } from '@earendil-works/pi-ai/api/transform-messages'
import { header, check } from './lib/harness.mjs'
import { createProbeSession } from './lib/session.mjs'

const root = '.probe-s0/tmp/p7'
const sessionDir = root + '/sessions'
const pathFile = root + '/artifact-path.txt'
if (!existsSync(pathFile)) { console.error('run p7-artifact.mjs first'); process.exit(1) }
const artifact = readFileSync(pathFile, 'utf8').trim()

// Each sub-experiment appends to the session it opens, so each works on its own copy.
const sessionFileFor = name => { const target = root + '/' + name + '.jsonl'; copyFileSync(artifact, target); return target }

// Precondition: the artifact must still carry the orphaned assistant message on its active path.
{
  const probe = SessionManager.open(artifact, sessionDir)
  const ok = probe.buildSessionContext().messages.some(m => m.role === 'assistant' && m.content.some(c => c.type === 'toolCall'))
  if (!ok) { console.error('artifact lost its orphaned assistant message; rerun p7-artifact.mjs'); process.exit(1) }
  console.log('artifact OK: ' + artifact + ' entries=' + probe.getEntryCount())
}

const readTool = {
  name: 'read', label: 'read', description: 'read tool', parameters: Type.Object({ path: Type.String() }), executionMode: 'parallel',
  async execute(id) { return { content: [{ type: 'text', text: 'executed ' + id }], details: {} } },
}

function describe(messages) {
  return messages.filter(m => m.role !== 'system').map(m => {
    if (m.role === 'assistant') return { role: 'assistant', stopReason: m.stopReason, toolCalls: m.content.filter(c => c.type === 'toolCall').map(c => c.id) }
    if (m.role === 'toolResult') return { role: 'toolResult', toolCallId: m.toolCallId, isError: m.isError, text: m.content.map(c => c.text).join('').slice(0, 32) }
    const text = typeof m.content === 'string' ? m.content : m.content.map(c => c.text ?? '').join('')
    return { role: m.role, text: String(text).slice(0, 32) }
  })
}

function orphanIds(messages) {
  const out = []
  let pending = []
  let answered = new Set()
  const close = () => { out.push(...pending.filter(id => !answered.has(id))); pending = []; answered = new Set() }
  for (const m of messages) {
    if (m.role === 'assistant') {
      close()
      if (m.stopReason === 'error' || m.stopReason === 'aborted') continue
      const calls = m.content.filter(c => c.type === 'toolCall').map(c => c.id)
      if (calls.length) { pending = calls; answered = new Set() }
    } else if (m.role === 'toolResult') answered.add(m.toolCallId)
    else if (m.role === 'user') close()
  }
  close()
  return out
}

const syntheticOf = (messages, model) => transformMessages(structuredClone(messages), model)
  .filter(m => m.role === 'toolResult' && m.content[0]?.text === 'No result provided')
const nonSystem = ms => ms.filter(m => m.role !== 'system')

// ---------------------------------------------------------------- (c) control: no repair
header('P7c  control: reopen the orphaned session and prompt (no repair)')
{
  const m = SessionManager.open(sessionFileFor('case-c'), sessionDir)
  const ctx = m.buildSessionContext()
  console.log('  SDK-built context:'); for (const d of describe(ctx.messages)) console.log('   ' + JSON.stringify(d))
  const orphans = orphanIds(ctx.messages)
  console.log('  orphaned toolCalls: ' + JSON.stringify(orphans))

  const { session, faux, model } = await createProbeSession({ cwd: root, sessionDir, customTools: [readTool], sessionManager: m })
  faux.script([fauxAssistantMessage('after orphan', { stopReason: 'stop' })])
  await session.prompt('carry on')
  await session.waitForIdle()
  const raw = nonSystem(session.agent.state.messages)
  console.log('\n  raw transcript the provider would receive:')
  for (const d of describe(raw)) console.log('   ' + JSON.stringify(d))
  const synthetic = syntheticOf(session.agent.state.messages, model)
  console.log('  transformMessages inserts ' + synthetic.length + ' synthetic result(s): ' + JSON.stringify(synthetic.map(x => x.toolCallId)))
  check('(c) a session with orphaned toolCalls still opens and runs', faux.requests.length === 1)
  check('(c) the raw transcript still contains the orphaned toolCalls', orphanIds(session.agent.state.messages).length === 2)
  check('(c) the provider layer repairs them with synthetic results', synthetic.length === 2)
}

// ---------------------------------------------------------------- (a) appendMessage the missing results
header('P7a  reopen + SessionManager.appendMessage() for each orphaned toolCallId')
let aMessages = null
{
  const m = SessionManager.open(sessionFileFor('case-a'), sessionDir)
  const before = m.buildSessionContext()
  const orphans = orphanIds(before.messages)
  const names = new Map(before.messages.filter(x => x.role === 'assistant').flatMap(x => x.content.filter(c => c.type === 'toolCall').map(c => [c.id, c.name])))
  console.log('  orphans: ' + JSON.stringify(orphans) + '  leaf=' + m.getLeafId())
  for (const id of orphans) {
    const entryId = m.appendMessage({
      role: 'toolResult', toolCallId: id, toolName: names.get(id) ?? 'read',
      content: [{ type: 'text', text: 'host recovery: outcome unknown, recorded as not executed' }],
      isError: true, timestamp: Date.now(),
    })
    console.log('  appendMessage(toolResult ' + id + ') -> entry ' + entryId)
  }
  const after = m.buildSessionContext()
  console.log('  context after repair:'); for (const d of describe(after.messages)) console.log('   ' + JSON.stringify(d))
  check('(a) no orphaned toolCalls remain right after the repair', orphanIds(after.messages).length === 0, JSON.stringify(orphanIds(after.messages)))

  const { session, faux, model } = await createProbeSession({ cwd: root, sessionDir, customTools: [readTool], sessionManager: m })
  faux.script([fauxAssistantMessage('after repair', { stopReason: 'stop' })])
  await session.prompt('carry on')
  await session.waitForIdle()
  aMessages = session.agent.state.messages
  console.log('\n  raw transcript the provider would receive:')
  for (const d of describe(nonSystem(aMessages))) console.log('   ' + JSON.stringify(d))
  const synthetic = syntheticOf(aMessages, model)
  const counts = {}
  for (const x of aMessages.filter(x => x.role === 'toolResult')) counts[x.toolCallId] = (counts[x.toolCallId] ?? 0) + 1
  check('(a) no duplicate toolResult per toolCallId', Object.values(counts).every(v => v === 1), JSON.stringify(counts))
  check('(a) the provider layer adds no synthetic result', synthetic.length === 0, JSON.stringify(synthetic.map(x => x.toolCallId)))
  check('(a) no orphaned toolCall reaches the provider', orphanIds(aMessages).length === 0, JSON.stringify(orphanIds(aMessages)))
  check('(a) the repaired results are positioned before the new user turn',
    (() => { const idx = aMessages.findIndex(x => x.role === 'toolResult'); const u = aMessages.findIndex(x => x.role === 'user' && x.content?.[0]?.text === 'carry on'); return idx >= 0 && idx < u })())
}

// ---------------------------------------------------------------- (b) branch() back past the assistant message
header('P7b  reopen + SessionManager.branch() to the entry before the orphaned assistant message')
let bMessages = null
{
  const m = SessionManager.open(sessionFileFor('case-b'), sessionDir)
  const assistantEntry = m.getEntries().find(e => e.type === 'message' && e.message?.role === 'assistant' && e.message.content.some(c => c.type === 'toolCall'))
  const parentId = assistantEntry.parentId
  console.log('  orphaned assistant entry: ' + assistantEntry.id + ' (parent ' + parentId + ')')
  m.branch(parentId)
  console.log('  leaf moved to ' + m.getLeafId())
  const after = m.buildSessionContext()
  console.log('  context after branch:'); for (const d of describe(after.messages)) console.log('   ' + JSON.stringify(d))
  check('(b) the orphaned assistant message is gone from the active path',
    !after.messages.some(x => x.role === 'assistant' && x.content.some(c => c.type === 'toolCall')))

  const { session, faux, model } = await createProbeSession({ cwd: root, sessionDir, customTools: [readTool], sessionManager: m })
  faux.script([fauxAssistantMessage('after branch', { stopReason: 'stop' })])
  await session.prompt('carry on')
  await session.waitForIdle()
  bMessages = session.agent.state.messages
  console.log('\n  raw transcript the provider would receive:')
  for (const d of describe(nonSystem(bMessages))) console.log('   ' + JSON.stringify(d))
  check('(b) no toolResult at all for the abandoned toolCalls', !bMessages.some(x => x.role === 'toolResult'))
  check('(b) the file stays append-only: the abandoned entry is still in it',
    m.getEntries().some(e => e.id === assistantEntry.id), 'entries=' + m.getEntryCount())
  check('(b) the provider layer needs no synthetic result', syntheticOf(bMessages, model).length === 0)
}

header('P7 summary')
console.log('  (a) active transcript: ' + JSON.stringify(describe(nonSystem(aMessages))))
console.log('  (b) active transcript: ' + JSON.stringify(describe(nonSystem(bMessages))))
check('both recovery paths leave a provider-valid transcript',
  orphanIds(aMessages).length === 0 && orphanIds(bMessages).length === 0)
console.log('\n  (a) keeps the assistant turn; the Host injects a result for each orphaned toolCall.')
console.log('  (b) removes the assistant turn from the active path; the model never learns it called those tools.')
