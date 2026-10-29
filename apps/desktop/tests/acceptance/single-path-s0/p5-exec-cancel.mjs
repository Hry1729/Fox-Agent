// P5 cancellation while a tool is executing: a slow read, then a write behind the barrier.
// Run: node services/agent-runtime/.probe-s0/p5-exec-cancel.mjs
import { Type } from '@earendil-works/pi-ai'
import { header, makeFaux, makeAgent, makeTimeline, HostScheduler, batchMessage, textMessage, now, sleep } from './lib/harness.mjs'

function makeTools({ scheduler, executed, writeEffects, writeChecksSignal, readHonoursSignal, readMs }) {
  const read = {
    name: 'read', label: 'read', description: 'read', parameters: Type.Object({ path: Type.String() }), executionMode: 'parallel',
    async execute(id, params, signal) {
      executed.push({ id, at: now() })
      await scheduler.acquireRead()
      try {
        if (readHonoursSignal) {
          // realistic abortable IO
          await new Promise((resolve) => {
            const t = setTimeout(resolve, readMs)
            signal?.addEventListener('abort', () => { clearTimeout(t); resolve() }, { once: true })
          })
          if (signal?.aborted) throw new Error('Operation aborted')
        } else {
          await sleep(readMs)   // ignores the signal on purpose
        }
        return { content: [{ type: 'text', text: 'read done' }], details: {} }
      } finally {
        scheduler.releaseRead()
      }
    },
  }
  const write = {
    name: 'write', label: 'write', description: 'write', parameters: Type.Object({ path: Type.String(), text: Type.String() }), executionMode: 'parallel',
    async execute(id, params, signal) {
      executed.push({ id, at: now() })
      return scheduler.withBarrier(async () => {
        if (writeChecksSignal && signal?.aborted) {
          return { content: [{ type: 'text', text: 'skipped: run aborted' }], details: {}, isError: true }
        }
        writeEffects.push({ id, at: now() })
        return { content: [{ type: 'text', text: 'wrote' }], details: {} }
      })
    },
  }
  return [read, write]
}

async function scenario(label, opts) {
  header(`P5${label}  abort during a ${opts.readMs}ms read; write behind the barrier (writeChecksSignal=${opts.writeChecksSignal}, readHonoursSignal=${opts.readHonoursSignal})`)
  const timeline = makeTimeline()
  const scheduler = new HostScheduler({ readLimit: 4 })
  const executed = []
  const writeEffects = []
  const tools = makeTools({ scheduler, executed, writeEffects, ...opts })

  const faux = makeFaux()
  faux.script([
    batchMessage([
      { id: 'call_slow_read', name: 'read', args: { path: 'slow' } },
      { id: 'call_write', name: 'write', args: { path: 'W.txt', text: 'x' } },
    ]),
    textMessage('done'),
  ])

  const { agent, events } = makeAgent({ faux, tools })
  let abortAt = null
  let abortedSignalSeen = null
  agent.subscribe(e => {
    if (e.type === 'tool_execution_start' && e.toolCallId === 'call_slow_read' && abortAt === null) {
      setTimeout(() => { abortAt = now(); timeline.mark('probe', 'agent.abort() called'); agent.abort() }, 100)
    }
  })

  const t0 = now()
  await agent.prompt('go')
  await agent.waitForIdle()
  const settledAt = now()

  const readResult = agent.state.messages.filter(m => m.role === 'toolResult' && m.toolCallId === 'call_slow_read').at(-1)
  const writeResult = agent.state.messages.filter(m => m.role === 'toolResult' && m.toolCallId === 'call_write').at(-1)

  console.log('\n-- timeline')
  for (const e of events.filter(e => ['tool_execution_start', 'tool_execution_end', 'agent_end'].includes(e.type))) {
    console.log(`  t+${String(e.t).padStart(9)}ms  ${e.type.padEnd(20)} ${e.toolCallId ?? ''} isError=${e.isError ?? ''}`)
  }
  console.log(`\n  abort called at t+${abortAt}ms`)
  console.log(`  execute() entered for: ${JSON.stringify(executed.map(e => e.id))}`)
  console.log(`  write EFFECTS performed: ${JSON.stringify(writeEffects.map(e => e.id))}`)
  console.log(`  read  result: isError=${readResult?.isError} text=${JSON.stringify(readResult?.content.map(c => c.text).join(''))}`)
  console.log(`  write result: isError=${writeResult?.isError} text=${JSON.stringify(writeResult?.content.map(c => c.text).join(''))}`)
  console.log(`  abort -> run settled: ${(settledAt - abortAt).toFixed(1)}ms (total run ${(settledAt - t0).toFixed(1)}ms)`)

  console.log('\n-- verdict')
  console.log(`  write execute() was entered: ${executed.some(e => e.id === 'call_write')}  (a Host that relies on "never enters execute" cannot rely on the SDK)`)
  console.log(`  write effect performed after abort: ${writeEffects.length > 0} (writeChecksSignal=${opts.writeChecksSignal})`)
  console.log(`  read result classified as error: ${readResult?.isError === true}`)
  console.log(`  the abort signal reached the tool: ${readResult?.isError === true || opts.readHonoursSignal === false}`)
  return { settledAt, abortAt, writeEffects, executed }
}

await scenario('a', { readMs: 600, writeChecksSignal: false, readHonoursSignal: true })
await scenario('b', { readMs: 600, writeChecksSignal: true, readHonoursSignal: true })
await scenario('c', { readMs: 600, writeChecksSignal: true, readHonoursSignal: false })

header('P5d  does the run settle before the tools do?')
{
  const scheduler = new HostScheduler({ readLimit: 4 })
  const executed = []
  const writeEffects = []
  const tools = makeTools({ scheduler, executed, writeEffects, readMs: 800, writeChecksSignal: true, readHonoursSignal: false })
  const faux = makeFaux()
  faux.script([
    batchMessage([
      { id: 'call_slow_read', name: 'read', args: { path: 'slow' } },
      { id: 'call_write', name: 'write', args: { path: 'W.txt', text: 'x' } },
    ]),
    textMessage('done'),
  ])
  const { agent, events } = makeAgent({ faux, tools })
  let abortAt = null
  agent.subscribe(e => {
    if (e.type === 'tool_execution_start' && e.toolCallId === 'call_slow_read' && abortAt === null) {
      setTimeout(() => { abortAt = now(); agent.abort() }, 50)
    }
  })
  await agent.prompt('go')
  await agent.waitForIdle()
  const end = events.find(e => e.type === 'agent_end')
  console.log(`  tool ignores the signal (800ms); abort at t+${abortAt}ms; agent_end at t+${end.t}ms`)
  console.log(`  => the run waited ${(end.t - abortAt).toFixed(1)}ms after abort because Promise.all is not abortable`)
}
