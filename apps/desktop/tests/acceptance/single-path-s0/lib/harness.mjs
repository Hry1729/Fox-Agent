// S0 probe harness: fake-model plumbing, event timeline, and the Host barrier scheduler.
// Public API used: pi-agent-core Agent + AgentOptions.streamFn, pi-ai faux provider.
import { Agent } from '@earendil-works/pi-agent-core'
import { createFauxCore, fauxAssistantMessage, fauxToolCall, Type } from '@earendil-works/pi-ai'

export const T0 = performance.now()
export const now = () => +(performance.now() - T0).toFixed(3)

export function makeTimeline() {
  const rows = []
  return {
    rows,
    mark(kind, label, extra = {}) {
      rows.push({ t: now(), kind, label, ...extra })
      return rows.at(-1)
    },
    dump() {
      for (const r of rows) {
        const extra = Object.keys(r).filter(k => !['t', 'kind', 'label'].includes(k))
        const tail = extra.length ? ' ' + extra.map(k => `${k}=${JSON.stringify(r[k])}`).join(' ') : ''
        console.log(`  t+${String(r.t).padStart(9)}ms  ${r.kind.padEnd(22)} ${r.label}${tail}`)
      }
    },
  }
}

export const sleep = ms => new Promise(r => setTimeout(r, ms))

/**
 * Host barrier scheduler.
 * - read-only calls (read/ls/find/grep) run at most `readLimit` at a time.
 * - write (and any tool flagged barrier) waits for every earlier call to settle and blocks
 *   every later call until it settles.
 * All tools declare executionMode:"parallel"; ordering is entirely this scheduler's job.
 */
export class HostScheduler {
  constructor({ readLimit = 4, timeline } = {}) {
    this.readLimit = readLimit
    this.timeline = timeline
    this.active = 0
    this.peak = 0
    this.readWaiters = []
    this.barrierWaiters = []
    this.barrierActive = false
    this.barrierChain = Promise.resolve()
    this.pendingBarriers = 0
    this.arrivals = []
  }
  _schedule() {
    if (this.barrierActive) {
      if (this.active === 0 && this.barrierWaiters.length) this.barrierWaiters.shift()()
      return
    }
    while (this.readWaiters.length && this.active < this.readLimit) {
      this.active++
      this.peak = Math.max(this.peak, this.active)
      this.readWaiters.shift()()
    }
  }
  async acquireRead() {
    if (!this.barrierActive && this.active < this.readLimit) {
      this.active++
      this.peak = Math.max(this.peak, this.active)
      return
    }
    await new Promise(resolve => this.readWaiters.push(resolve))
  }
  releaseRead() {
    this.active--
    this._schedule()
  }
  /** Exclusive stage: waits for in-flight readers, blocks new ones until the body resolves. */
  async withBarrier(body) {
    const previous = this.barrierChain
    let release
    this.barrierChain = new Promise(r => { release = r })
    this.pendingBarriers++
    this.barrierActive = true
    try {
      await previous
      if (this.active > 0) await new Promise(resolve => this.barrierWaiters.push(resolve))
      return await body()
    } finally {
      this.pendingBarriers--
      release()
      if (this.pendingBarriers === 0) {
        this.barrierActive = false
        this._schedule()
      }
    }
  }
}

const readSchema = Type.Object({ path: Type.String({ description: 'path' }) })
const writeSchema = Type.Object({ path: Type.String(), text: Type.String() })

/**
 * Build the probe tool set. Every tool is executionMode:"parallel".
 * `execute` records arrival/start/end so the timeline shows real overlap.
 */
export function makeHostTools({ scheduler, timeline, readDelayMs = 40, writeDir, onExecute, signalProbe }) {
  const record = []
  const tool = (name, kind, schema, run) => ({
    name,
    label: name,
    description: `${kind} tool ${name}`,
    parameters: schema,
    executionMode: 'parallel',
    async execute(toolCallId, params, signal, onUpdate) {
      const arrivedAt = now()
      scheduler.arrivals.push({ toolCallId, name, arrivedAt })
      if (onExecute) onExecute({ toolCallId, name, kind, params, signal })
      const entry = { toolCallId, name, kind, arrivedAt, startAt: null, endAt: null, sawAbortSignal: false }
      record.push(entry)
      try {
        if (kind === 'barrier') {
          entry.barrierWaitFrom = arrivedAt
          return await scheduler.withBarrier(async () => {
            entry.barrierStart = now()
            timeline.mark('sched', `${name} enters barrier (exclusive stage)`, { id: toolCallId })
            entry.startAt = now()
            try {
              return await run(toolCallId, params, signal)
            } finally {
              entry.endAt = now()
              timeline.mark('sched', `${name} leaves barrier`, { id: toolCallId })
            }
          })
        }
        await scheduler.acquireRead()
        entry.startAt = now()
        try {
          if (signal?.aborted) entry.sawAbortSignal = true
          return await run(toolCallId, params, signal)
        } finally {
          entry.endAt = now()
          scheduler.releaseRead()
        }
      } finally {
        if (entry.sawAbortSignal === false && signal?.aborted) entry.sawAbortSignal = true
      }
    },
  })

  const readBody = (id, p, signal) => async () => {
    // Emulate real IO latency while remaining abortable, like a real tool would be.
    await abortableSleep(readDelayMs, signal)
    return { content: [{ type: 'text', text: `ok ${id}` }], details: {} }
  }

  const tools = [
    tool('read', 'read', readSchema, (id, p, s) => readBody(id, p, s)()),
    tool('ls', 'read', readSchema, (id, p, s) => readBody(id, p, s)()),
    tool('find', 'read', readSchema, (id, p, s) => readBody(id, p, s)()),
    tool('grep', 'read', readSchema, (id, p, s) => readBody(id, p, s)()),
    tool('write', 'barrier', writeSchema, async (id, params) => {
      const { mkdirSync, writeFileSync } = await import('node:fs')
      mkdirSync(writeDir, { recursive: true })
      writeFileSync(`${writeDir}/${params.path.replace(/[\\/:]/g, '_')}`, params.text ?? '')
      return { content: [{ type: 'text', text: `wrote ${params.path}` }], details: {} }
    }),
  ]
  return { tools, record }
}

export function abortableSleep(ms, signal) {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(new Error('Operation aborted'))
    const t = setTimeout(resolve, ms)
    signal?.addEventListener('abort', () => { clearTimeout(t); reject(new Error('Operation aborted')) }, { once: true })
  })
}

/** Scripted fake model over the public pi-ai faux provider. */
export function makeFaux({ tokensPerSecond } = {}) {
  const core = createFauxCore({
    api: 'faux-s0',
    provider: 'faux-s0',
    models: [{ id: 'faux-1', name: 'Faux 1', contextWindow: 128000, maxTokens: 16384 }],
    ...(tokensPerSecond ? { tokensPerSecond } : {}),
  })
  const model = core.getModel()
  const requests = []
  /** Queue one scripted assistant message per model request. */
  const script = steps => core.setResponses(steps.map(s => (typeof s === 'function'
    ? (context, options, state, requestModel) => {
        requests.push({ t: now(), messageCount: context.messages.length, messages: structuredClone(context.messages), tools: (context.tools ?? []).map(t => t.name) })
        return s(context, options, state, requestModel)
      }
    : (context, options, state, requestModel) => {
        requests.push({ t: now(), messageCount: context.messages.length, messages: structuredClone(context.messages), tools: (context.tools ?? []).map(t => t.name) })
        return s
      })))
  return { core, model, streamFn: core.streamSimple, script, requests, state: core.state }
}

export function batchMessage(specs, opts = {}) {
  return fauxAssistantMessage(specs.map(s => fauxToolCall(s.name, s.args, { id: s.id })), { stopReason: 'toolUse', ...opts })
}

export function textMessage(text, opts = {}) {
  return fauxAssistantMessage(text, { stopReason: 'stop', ...opts })
}

/** Build an Agent wired to the fake model, collecting every event with a timestamp. */
export function makeAgent({ faux, tools, systemPrompt = 'S0 probe', options = {} }) {
  const events = []
  const agent = new Agent({
    initialState: { systemPrompt, model: faux.model, tools, thinkingLevel: 'off' },
    streamFn: faux.streamFn,
    toolExecution: 'parallel',
    ...options,
  })
  agent.subscribe(event => {
    events.push({ t: now(), type: event.type, ...describeEvent(event) })
  })
  return { agent, events }
}

export function describeEvent(event) {
  switch (event.type) {
    case 'message_end':
    case 'message_start':
    case 'turn_end': {
      const m = event.message
      const ids = Array.isArray(m?.content) ? m.content.filter(c => c.type === 'toolCall').map(c => c.id) : []
      return { role: m?.role, stopReason: m?.stopReason, toolCallIds: ids, toolCallId: m?.toolCallId }
    }
    case 'tool_execution_start': return { toolCallId: event.toolCallId, toolName: event.toolName }
    case 'tool_execution_end': return { toolCallId: event.toolCallId, toolName: event.toolName, isError: event.isError }
    case 'agent_end': return { messageCount: event.messages?.length }
    default: return {}
  }
}

export function header(title) {
  console.log('\n' + '='.repeat(96))
  console.log(title)
  console.log('='.repeat(96))
}

export function check(label, pass, detail = '') {
  console.log(`${pass ? 'PASS' : 'FAIL'}  ${label}${detail ? '  (' + detail + ')' : ''}`)
  return pass
}

/** Print a compact view of the messages the fake model actually received. */
export function summarizeRequest(messages) {
  return messages.map(m => {
    if (m.role === 'system') return { role: 'system', len: (typeof m.content === 'string' ? m.content : JSON.stringify(m.content)).length }
    if (m.role === 'assistant') return { role: 'assistant', stopReason: m.stopReason, toolCalls: m.content.filter(c => c.type === 'toolCall').map(c => c.id), text: m.content.filter(c => c.type === 'text').map(c => c.text).join('') }
    if (m.role === 'toolResult') return { role: 'toolResult', toolCallId: m.toolCallId, toolName: m.toolName, isError: m.isError, text: m.content.filter(c => c.type === 'text').map(c => c.text).join('').slice(0, 60) }
    return { role: m.role, content: typeof m.content === 'string' ? m.content : JSON.stringify(m.content).slice(0, 60) }
  })
}
