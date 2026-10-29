// P3 positions with no execute(): unknown tool, schema-invalid args, blocked by beforeToolCall.
// Compares (a) tool_execution_end events vs (b) pre-validation at registration vs (c) other public routes.
// Run: node services/agent-runtime/.probe-s0/p3-missing-execute.mjs
import { Value } from 'typebox/value'
import { Type } from '@earendil-works/pi-ai'
import { header, makeFaux, makeAgent, makeTimeline, batchMessage, textMessage, now, sleep, abortableSleep } from './lib/harness.mjs'

const BLOCKED_ID = 'call_blocked'
const WRITE_ID = 'call_write_D'
const READ_SCHEMA = Type.Object({ path: Type.String() })
const WRITE_SCHEMA = Type.Object({ path: Type.String(), text: Type.String() })

function makeBatch() {
  return batchMessage([
    { id: 'call_read_A', name: 'read', args: { path: 'A' } },
    { id: 'call_unknown', name: 'zzz_unknown_tool', args: { path: 'U' } },
    { id: 'call_badargs', name: 'read', args: {} },                 // missing required "path"
    { id: BLOCKED_ID, name: 'read', args: { path: 'B' } },           // blocked by beforeToolCall
    { id: WRITE_ID, name: 'write', args: { path: 'D.txt', text: 'x' } },
  ])
}

function makeTools({ writeDir, executed, onWrite }) {
  const readTool = {
    name: 'read', label: 'read', description: 'read', parameters: READ_SCHEMA, executionMode: 'parallel',
    async execute(id, params, signal) {
      executed.push({ id, at: now(), params })
      await abortableSleep(60, signal)
      return { content: [{ type: 'text', text: `ok ${id}` }], details: {} }
    },
  }
  const writeTool = {
    name: 'write', label: 'write', description: 'write', parameters: WRITE_SCHEMA, executionMode: 'parallel',
    async execute(id, params) {
      executed.push({ id, at: now(), params })
      await onWrite()
      const { mkdirSync, writeFileSync } = await import('node:fs')
      mkdirSync(writeDir, { recursive: true })
      writeFileSync(`${writeDir}/p3-D.txt`, params.text ?? '')
      return { content: [{ type: 'text', text: 'wrote D' }], details: {} }
    },
  }
  return [readTool, writeTool]
}

function makeLedger() {
  const state = { order: [], byId: new Map(), settled: new Set(), waiters: [], registeredAt: null, predicted: [] }
  const settle = id => { if (!state.settled.has(id)) { state.settled.add(id); state.waiters.splice(0).forEach(fn => fn()) } }
  const waitLower = async (id, timeoutMs) => {
    const me = state.byId.get(id)
    if (!me) return { released: false, waited: 0, error: 'position not registered' }
    const need = state.order.filter(o => o.index < me.index).map(o => o.id)
    const ready = () => need.every(n => state.settled.has(n))
    if (ready()) return { released: true, waited: 0 }
    const from = now()
    const outcome = await Promise.race([
      new Promise(resolve => { const check = () => (ready() ? resolve('released') : state.waiters.push(check)); state.waiters.push(check) }),
      sleep(timeoutMs).then(() => 'timeout'),
    ])
    return { released: outcome === 'released', waited: +(now() - from).toFixed(3) }
  }
  const register = (assistantMessage, tools) => {
    if (state.registeredAt) return
    state.registeredAt = now()
    const toolCalls = assistantMessage.content.filter(c => c.type === 'toolCall')
    state.order = toolCalls.map((tc, index) => ({ index, id: tc.id, name: tc.name }))
    for (const o of state.order) state.byId.set(o.id, o)
    state.predicted = toolCalls.map(tc => {
      const tool = tools.find(t => t.name === tc.name)
      if (!tool) return { id: tc.id, status: 'unknown-tool' }
      // Host-side pre-validation with the same tool table, the obvious way: plain schema check.
      if (!Value.Check(tool.parameters, tc.arguments)) return { id: tc.id, status: 'value-check-fails' }
      if (tc.id === BLOCKED_ID) return { id: tc.id, status: 'blocked-by-policy' }
      return { id: tc.id, status: 'will-execute' }
    })
  }
  return { state, settle, waitLower, register }
}

async function run(mode) {
  header(`P3${mode}  settlement source: ${mode === 'a' ? '(a) tool_execution_end events only' : '(b) registration-time pre-validation + events for the executed ones'}`)
  const writeDir = new URL('./tmp/', import.meta.url).pathname.replace(/^\//, '').replace(/\//g, '\\')
  const executed = []
  const ledger = makeLedger()
  let writeResult = null
  const tools = makeTools({ writeDir, executed, onWrite: async () => { writeResult = await ledger.waitLower(WRITE_ID, 2000); if (mode === 'b') await sleep(0) } })
  const faux = makeFaux()
  faux.script([makeBatch(), textMessage('done')])

  const beforeToolCall = async ({ assistantMessage, toolCall }) => {
    ledger.register(assistantMessage, tools)
    if (mode === 'b') {
      for (const p of ledger.state.predicted) if (p.status !== 'will-execute') ledger.settle(p.id)
    }
    if (toolCall.id === BLOCKED_ID) return { block: true, reason: 'blocked by probe policy' }
    return undefined
  }

  const afterCalls = []
  const { agent, events } = makeAgent({ faux, tools, options: { beforeToolCall, afterToolCall: async ({ toolCall }) => { afterCalls.push(toolCall.id); return undefined } } })
  agent.subscribe(e => { if (e.type === 'tool_execution_end') ledger.settle(e.toolCallId) })
  await agent.prompt('go')
  await agent.waitForIdle()

  const startEvents = events.filter(e => e.type === 'tool_execution_start')
  const endEvents = events.filter(e => e.type === 'tool_execution_end')
  const toolResults = events.filter(e => e.type === 'message_end' && e.role === 'toolResult')

  console.log(`-- registered order at first beforeToolCall: ${JSON.stringify(ledger.state.order.map(o => `${o.index}:${o.id}`))}`)
  if (mode === 'b') console.log(`-- pre-validation verdicts: ${JSON.stringify(ledger.state.predicted.map(p => `${p.id}=${p.status}`))}`)
  console.log('\n-- tool_execution_start / end')
  for (const s of startEvents) {
    const e = endEvents.find(x => x.toolCallId === s.toolCallId)
    console.log(`  ${s.toolCallId.padEnd(20)} start=t+${String(s.t).padStart(9)}  end=${e ? 't+' + String(e.t).padStart(9) : 'NEVER'}  isError=${e?.isError}`)
  }
  console.log('\n-- execute() called for: ' + JSON.stringify(executed.map(e => e.id)))
  console.log('-- toolResult emitted for: ' + JSON.stringify(toolResults.map(e => e.toolCallId)))
  console.log(`-- write barrier: ${JSON.stringify(writeResult)} ; write execute at t+${executed.find(e => e.id === WRITE_ID)?.at}`)

  const noExecute = ['call_unknown', 'call_badargs', BLOCKED_ID]
  const allHaveStart = noExecute.every(id => startEvents.some(s => s.toolCallId === id))
  const allHaveEnd = noExecute.every(id => endEvents.some(s => s.toolCallId === id))
  const noneExecuted = noExecute.every(id => !executed.some(e => e.id === id))
  console.log('\n-- verdict')
  console.log(`${allHaveStart && allHaveEnd ? 'YES' : 'NO '}  every no-execute position still emits tool_execution_start AND tool_execution_end (isError=true)`)
  console.log(`${noneExecuted ? 'YES' : 'NO '}  none of the three reached execute()`)
  console.log('  afterToolCall ran for: ' + JSON.stringify(afterCalls))
  console.log((afterCalls.length === 2 && !afterCalls.includes('call_unknown') && !afterCalls.includes('call_badargs') && !afterCalls.includes(BLOCKED_ID) ? 'YES' : 'NO ') + '  afterToolCall is skipped for every no-execute position')
  if (mode === 'a') console.log(`${writeResult?.released ? 'PASS' : 'FAIL'}  (a) event-driven settlement released the barrier (waited ${writeResult?.waited}ms)`)
  if (mode === 'b') console.log(`${writeResult?.released ? 'PASS' : 'FAIL'}  (b) pre-validated + event settlement released the barrier (waited ${writeResult?.waited}ms)`)
  return { events, executed, writeResult, ledger, endEvents, toolResults }
}

const a = await run('a')
const b = await run('b')

header('P3c  argument-validation reality check (Value.Convert runs before the schema check)')
{
  const cases = [
    { label: 'missing required property', args: {}, check: Value.Check(READ_SCHEMA, {}) },
    { label: 'number where string is required', args: { path: 123 }, check: Value.Check(READ_SCHEMA, { path: 123 }) },
    { label: 'null where string is required', args: { path: null }, check: Value.Check(READ_SCHEMA, { path: null }) },
    { label: 'array where string is required', args: { path: ['a'] }, check: Value.Check(READ_SCHEMA, { path: ['a'] }) },
    { label: 'extra unknown property', args: { path: 'x', extra: 1 }, check: Value.Check(READ_SCHEMA, { path: 'x', extra: 1 }) },
  ]
  for (const c of cases) {
    const executed = []
    const tools = makeTools({ writeDir: new URL('./tmp/', import.meta.url).pathname.replace(/^\//, '').replace(/\//g, '\\'), executed, onWrite: async () => {} })
    const faux = makeFaux()
    faux.script([batchMessage([{ id: 'call_x', name: 'read', args: c.args }]), textMessage('done')])
    const { agent } = makeAgent({ faux, tools })
    await agent.prompt('go')
    await agent.waitForIdle()
    const res = agent.state.messages.filter(m => m.role === 'toolResult').at(-1)
    const verdict = executed.length ? 'EXECUTED' : (res?.isError ? 'REJECTED by SDK' : 'other')
    console.log(`  ${c.label.padEnd(34)} Value.Check=${String(c.check).padEnd(5)} -> ${verdict}${res?.isError ? '  msg=' + JSON.stringify(res.content.map(x => x.text).join(' ').slice(0, 70)) : ''}`)
  }
}

console.log('\n-- P3c notes')
console.log(`  toolResult messages arrive only after the whole batch settles: first tool_execution_end=t+${a.endEvents[0].t}ms, first toolResult=t+${a.toolResults[0].t}ms`)
console.log('  agent.state.pendingToolCalls is derived only from tool_execution_start/end (agent.js:412-420), so it cannot')
console.log('  distinguish "settled" from "never entered the batch".')

