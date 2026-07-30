export const WORK_EVENT_SCHEMA_VERSION = 1

export const WORK_EVENT_TYPES = Object.freeze([
  'goal.proposed',
  'goal.activated',
  'goal.blocked',
  'goal.completed',
  'goal.cancelled',
  'task.created',
  'task.started',
  'task.completed',
  'task.blocked',
  'task.interrupted',
  'evidence.added',
  'evidence.validated',
])

const knownTypes = new Set(WORK_EVENT_TYPES)

export function validateWorkEvent(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return 'work event must be an object'
  if (!knownTypes.has(value.type)) return null
  if (value.schemaVersion !== WORK_EVENT_SCHEMA_VERSION) return 'unsupported work event schema version'
  if (typeof value.conversationId !== 'string' || !value.conversationId.trim()) return 'conversationId is required'
  for (const field of ['goalId', 'taskId', 'runId', 'traceId', 'spanId']) {
    if (value[field] !== null && value[field] !== undefined && typeof value[field] !== 'string') {
      return `${field} must be a string or null`
    }
  }
  if (!Number.isSafeInteger(value.sequence) || value.sequence < 0) return 'sequence must be a non-negative integer'
  if (typeof value.timestamp !== 'string' || Number.isNaN(Date.parse(value.timestamp))) return 'timestamp must be an ISO-8601 timestamp'
  return null
}

export function createWorkEvent({ type, conversationId, goalId = null, taskId = null, runId = null,
  traceId = null, spanId = null, sequence, timestamp = new Date().toISOString(), data = {} }) {
  const event = {
    type,
    schemaVersion: WORK_EVENT_SCHEMA_VERSION,
    conversationId,
    goalId,
    taskId,
    runId,
    traceId,
    spanId,
    sequence,
    timestamp,
    data,
  }
  const error = validateWorkEvent(event)
  if (error) throw new Error(error)
  return event
}

export function createWorkEventDeduper() {
  const seen = new Set()
  return {
    accept(event) {
      if (!knownTypes.has(event?.type)) return false
      const error = validateWorkEvent(event)
      if (error) throw new Error(error)
      const key = `${event.conversationId}\u0000${event.sequence}`
      if (seen.has(key)) return false
      seen.add(key)
      return true
    },
  }
}
