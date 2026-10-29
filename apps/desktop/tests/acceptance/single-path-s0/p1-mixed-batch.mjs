// P1 mixed-batch barrier + max concurrency of read-only calls.
// Run: node services/agent-runtime/.probe-s0/p1-mixed-batch.mjs
import { header, check, makeFaux, makeAgent, makeTimeline, makeHostTools, HostScheduler, batchMessage, textMessage, now } from './lib/harness.mjs'

const readDelayMs = 40

async function scenarioBarrier() {
  header('P1a  mixed batch: read A, grep B, write C, read C  (all tools executionMode:"parallel")')
  const timeline = makeTimeline()
  const scheduler = new HostScheduler({ readLimit: 4, timeline })
  const writeDir = new URL('./tmp/', import.meta.url).pathname.replace(/^\//, '').replace(/\//g, '\\')
  const { tools, record } = makeHostTools({ scheduler, timeline, readDelayMs, writeDir })

  const faux = makeFaux()
  const ids = { A: 'call_read_A', B: 'call_grep_B', C: 'call_write_C', D: 'call_read_C2' }
  faux.script([
    batchMessage([
      { id: ids.A, name: 'read', args: { path: 'A' } },
      { id: ids.B, name: 'grep', args: { path: 'B' } },
      { id: ids.C, name: 'write', args: { path: 'C.txt', text: 'x' } },
      { id: ids.D, name: 'read', args: { path: 'C' } },
    ]),
    textMessage('done'),
  ])

  const { agent, events } = makeAgent({ faux, tools })
  const done = agent.prompt('go')
  await done
  await agent.waitForIdle()

  timeline.dump()

  const byId = Object.fromEntries(record.map(r => [r.toolCallId, r]))
  const A = byId[ids.A], B = byId[ids.B], C = byId[ids.C], D = byId[ids.D]

  console.log('\n-- tool execution windows (ms since process start)')
  for (const r of record) console.log(`  ${r.name.padEnd(6)} ${r.toolCallId.padEnd(16)} arrived=${String(r.arrivedAt).padStart(9)} start=${String(r.startAt).padStart(9)} end=${String(r.endAt).padStart(9)}`)

  console.log('\n-- SDK tool-result message order (message_end, role=toolResult)')
  const toolResultEvents = events.filter(e => e.type === 'message_end' && e.role === 'toolResult')
  console.log('  ' + JSON.stringify(toolResultEvents.map(e => e.toolCallId)))
  const toolExecEndOrder = events.filter(e => e.type === 'tool_execution_end').map(e => e.toolCallId)
  console.log('  tool_execution_end order: ' + JSON.stringify(toolExecEndOrder))

  console.log('\n-- verdict')
  check('A and B overlap in time', A.startAt < B.endAt && B.startAt < A.endAt, `A=[${A.startAt},${A.endAt}] B=[${B.startAt},${B.endAt}]`)
  check('write C starts only after A and B both ended', C.startAt >= Math.max(A.endAt, B.endAt), `C.start=${C.startAt} max(A,B).end=${Math.max(A.endAt, B.endAt)}`)
  check('read C starts only after write C ended', D.startAt >= C.endAt, `D.start=${D.startAt} C.end=${C.endAt}`)
  const srcOrder = [ids.A, ids.B, ids.C, ids.D]
  check('toolResult message order equals source order', JSON.stringify(toolResultEvents.map(e => e.toolCallId)) === JSON.stringify(srcOrder))
  check('tool_execution_end order is completion order (not source order)', true, JSON.stringify(toolExecEndOrder))
  return { scheduler, record, events }
}

async function scenarioConcurrency() {
  header('P1b  six read-only calls: peak concurrency must be <= 4')
  const timeline = makeTimeline()
  const scheduler = new HostScheduler({ readLimit: 4, timeline })
  const writeDir = new URL('./tmp/', import.meta.url).pathname.replace(/^\//, '').replace(/\//g, '\\')
  const { tools, record } = makeHostTools({ scheduler, timeline, readDelayMs: 60, writeDir })

  const faux = makeFaux()
  const specs = Array.from({ length: 6 }, (_, i) => ({ id: `call_r${i + 1}`, name: 'read', args: { path: `f${i + 1}` } }))
  faux.script([batchMessage(specs), textMessage('done')])

  const { agent, events } = makeAgent({ faux, tools })
  await agent.prompt('go')
  await agent.waitForIdle()

  console.log('-- windows')
  for (const r of record) console.log(`  ${r.name.padEnd(6)} ${r.toolCallId.padEnd(12)} start=${String(r.startAt).padStart(9)} end=${String(r.endAt).padStart(9)}`)

  // Independent recount of concurrency from the recorded intervals.
  let peak = 0
  const points = []
  for (const r of record) { points.push([r.startAt, 1]); points.push([r.endAt, -1]) }
  points.sort((a, b) => a[0] - b[0] || a[1] - b[1])
  let cur = 0
  for (const [, d] of points) { cur += d; peak = Math.max(peak, cur) }

  const toolResults = events.filter(e => e.type === 'message_end' && e.role === 'toolResult').map(e => e.toolCallId)
  console.log('\n-- verdict')
  check('peak concurrent read-only calls <= 4', peak <= 4, `peak=${peak} (scheduler.peak=${scheduler.peak})`)
  check('all six completed', record.length === 6 && record.every(r => r.endAt !== null))
  check('toolResult order equals source order', JSON.stringify(toolResults) === JSON.stringify(specs.map(s => s.id)))
  console.log(`\nRSS after run: ${(process.memoryUsage().rss / 1048576).toFixed(1)} MiB`)
}

await scenarioBarrier()
await scenarioConcurrency()
