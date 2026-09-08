import { PROTOCOL_NAME, PROTOCOL_VERSION } from '../../../packages/fox-engine-protocol/index.mjs'
export { PROTOCOL_NAME, PROTOCOL_VERSION }

let messageSequence = 0

export function createEnvelope(kind, type, fields = {}) {
  messageSequence += 1

  return {
    protocol: PROTOCOL_NAME,
    version: PROTOCOL_VERSION,
    kind,
    id: `runtime-${Date.now()}-${messageSequence}`,
    timestamp: new Date().toISOString(),
    type,
    ...fields,
  }
}

export function validateEnvelope(value) {
  if (!value || typeof value !== 'object') return 'message must be an object'
  if (value.protocol !== PROTOCOL_NAME) return 'unsupported protocol'
  if (value.version !== PROTOCOL_VERSION) return 'unsupported protocol version'
  if (value.kind !== 'request') return 'runtime accepts request messages only'
  if (typeof value.id !== 'string' || !value.id) return 'request id is required'
  if (typeof value.type !== 'string' || !value.type) return 'request type is required'
  return null
}
