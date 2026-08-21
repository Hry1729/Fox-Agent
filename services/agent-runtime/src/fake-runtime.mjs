import { createInterface } from 'node:readline'
import { readFile, writeFile } from 'node:fs/promises'
import {
  PROTOCOL_NAME,
  PROTOCOL_VERSION,
  createEnvelope,
  validateEnvelope,
} from './protocol.mjs'
import { createCapabilityManifest, RUNTIME_TOOL_CATALOG } from './runtime-contract.mjs'
import { createWorkEvent } from './work-events.mjs'

const sessions = new Map()
const activeRuns = new Map()
const pendingHostRequests = new Map()

function write(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`)
}

function respond(request, type, payload = {}) {
  write(createEnvelope('response', type, {
    requestId: request.id,
    conversationId: request.conversationId ?? null,
    runtimeSessionId: request.runtimeSessionId ?? null,
    runId: request.runId ?? null,
    payload,
  }))
}

function emitRuntimeEvent(request, seq, eventType, payload = {}) {
  write(createEnvelope('event', 'runtime_event', {
    conversationId: request.conversationId,
    runtimeSessionId: request.runtimeSessionId,
    runId: request.runId,
    seq,
    payload: {
      type: eventType,
      ...payload,
    },
  }))
}

function requestHost(request, type, payload) {
  const message = createEnvelope('request', type, {
    conversationId: request.conversationId,
    runtimeSessionId: request.runtimeSessionId,
    runId: request.runId,
    payload,
  })
  write(message)
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      pendingHostRequests.delete(message.id)
      reject(new Error(`host request timed out: ${type}`))
    }, 5_000)
    pendingHostRequests.set(message.id, { resolve, reject, timeout })
  })
}

function fakeAnswer(text) {
  const subject = text.trim() || 'this request'
  return `Fox Runtime foundation is connected. I received: ${subject}`
}

async function streamPrompt(request) {
  const controller = { cancelled: false }
  activeRuns.set(request.runId, controller)

  let seq = 1
  emitRuntimeEvent(request, seq++, 'run.started', { model: request.payload?.model ?? 'fake-model' })
  emitRuntimeEvent(request, seq++, 'message.started', { role: 'assistant', kind: 'text' })

  if (request.payload?.toolProbe) {
    emitRuntimeEvent(request, seq++, 'tool.started', {
      toolCallId: 'fake-tool-probe',
      tool: request.payload.toolProbe.tool,
      input: request.payload.toolProbe.input,
    })
    const preflight = await requestHost(request, 'tool.preflight', request.payload.toolProbe)
    emitRuntimeEvent(request, seq++, 'tool.completed', {
      toolCallId: 'fake-tool-probe',
      tool: request.payload.toolProbe.tool,
      decision: preflight.payload?.decision ?? 'block',
    })
  }

  if (request.payload?.workLoopProbe) {
    const probe = request.payload.workLoopProbe
    emitRuntimeEvent(request, seq++, 'tool.started', {
      toolCallId: 'fake-work-loop-probe',
      tool: probe.tool,
      input: probe.input,
    })
    const response = await requestHost(request, 'tool.execute', {
      toolCallId: 'fake-work-loop-probe',
      tool: probe.tool,
      input: probe.input,
    })
    if (response.payload?.isError || !response.payload?.result) {
      throw new Error(response.payload?.error || 'fake work-loop probe failed')
    }
    emitRuntimeEvent(request, seq++, 'tool.completed', {
      toolCallId: 'fake-work-loop-probe',
      tool: probe.tool,
      isError: false,
    })
    const details = response.payload.result.details ?? {}
    const goal = details.goal ?? null
    if (probe.tool === 'goal_propose' && goal?.id) {
      const event = details.workEvents?.[0] ?? createWorkEvent({
          type: 'goal.proposed',
          conversationId: request.conversationId,
          goalId: goal.id,
          runId: request.runId,
          sequence: probe.sequence ?? 1,
          data: { goal },
        })
      emitRuntimeEvent(request, seq++, 'goal.proposed', event)
    }
  }

  const words = fakeAnswer(request.payload?.text ?? '').split(' ')
  for (const word of words) {
    await new Promise((resolve) => setTimeout(resolve, 35))
    if (controller.cancelled) {
      emitRuntimeEvent(request, seq++, 'run.cancelled')
      activeRuns.delete(request.runId)
      return
    }
    emitRuntimeEvent(request, seq++, 'message.delta', { delta: `${word} ` })
  }

  emitRuntimeEvent(request, seq++, 'message.completed')
  emitRuntimeEvent(request, seq++, 'usage.updated', {
    inputTokens: Math.max(1, Math.ceil((request.payload?.text?.length ?? 0) / 4)),
    outputTokens: words.length * 2,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
  })
  emitRuntimeEvent(request, seq++, 'run.completed')
  activeRuns.delete(request.runId)
}

async function handleRequest(request) {
  const validationError = validateEnvelope(request)
  if (validationError) {
    write(createEnvelope('event', 'fatal_error', {
      payload: { code: 'protocol.invalid_message', message: validationError },
    }))
    return
  }

  switch (request.type) {
    case 'initialize':
      {
      const workLoop = request.payload?.workLoop === true
      respond(request, 'ready', {
        protocol: PROTOCOL_NAME,
        protocolVersion: PROTOCOL_VERSION,
        runtime: 'fox-fake-runtime',
        runtimeVersion: '0.1.0',
        capabilities: createCapabilityManifest({
          reasoning: false,
          toolApproval: false,
          contextCompaction: false,
          workLoop,
          tools: workLoop
            ? RUNTIME_TOOL_CATALOG.filter(({ category }) => category === 'work').map((tool) => ({ ...tool }))
            : [],
        }),
      })
      break
      }
    case 'create_session': {
      const runtimeSessionId = request.runtimeSessionId || `fake-session-${request.conversationId}`
      const session = { runtimeSessionId, conversationId: request.conversationId, updatedAt: new Date().toISOString() }
      sessions.set(runtimeSessionId, session)
      if (request.payload?.sessionPath) await writeFile(request.payload.sessionPath, JSON.stringify(session), 'utf8')
      respond({ ...request, runtimeSessionId }, 'session_created', { runtimeSessionId })
      break
    }
    case 'resume_session': {
      let session = sessions.get(request.runtimeSessionId)
      if (!session && request.payload?.sessionPath) {
        try {
          session = JSON.parse(await readFile(request.payload.sessionPath, 'utf8'))
          sessions.set(request.runtimeSessionId, session)
        } catch {
          session = null
        }
      }
      if (session?.runtimeSessionId === request.runtimeSessionId) {
        respond(request, 'session_created', { runtimeSessionId: request.runtimeSessionId })
      } else {
        respond(request, 'request_failed', {
          code: 'runtime.session_not_found',
          message: 'The fake runtime session does not exist.',
        })
      }
      break
    }
    case 'prompt':
      respond(request, 'request_succeeded')
      void streamPrompt(request)
      break
    case 'cancel': {
      const run = activeRuns.get(request.runId)
      if (run) run.cancelled = true
      respond(request, 'request_succeeded', { cancelling: Boolean(run) })
      break
    }
    case 'shutdown':
      respond(request, 'request_succeeded')
      setTimeout(() => process.exit(0), 10)
      break
    default:
      respond(request, 'request_failed', {
        code: 'protocol.unknown_request',
        message: `Unknown request type: ${request.type}`,
      })
  }
}

const input = createInterface({ input: process.stdin, crlfDelay: Infinity })
input.on('line', (line) => {
  try {
    const message = JSON.parse(line)
    if (message.kind === 'response' && message.requestId) {
      const pending = pendingHostRequests.get(message.requestId)
      if (pending) {
        clearTimeout(pending.timeout)
        pendingHostRequests.delete(message.requestId)
        pending.resolve(message)
      }
      return
    }
    void handleRequest(message)
  } catch (error) {
    process.stderr.write(`[fox-fake-runtime] ${error instanceof Error ? error.message : String(error)}\n`)
  }
})
