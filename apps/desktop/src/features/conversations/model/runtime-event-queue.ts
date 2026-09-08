import type { RuntimeEventNotification } from './types'

const pacedEventTypes = new Set(['message.delta', 'reasoning.delta'])

export function enqueueRuntimeEvent(queue: RuntimeEventNotification[], notification: RuntimeEventNotification) {
  const previous = queue.at(-1)
  if (
    previous
    && pacedEventTypes.has(notification.event.type)
    && previous.event.type === notification.event.type
    && previous.runId === notification.runId
    && previous.conversationId === notification.conversationId
    && typeof previous.event.delta === 'string'
    && typeof notification.event.delta === 'string'
  ) {
    queue[queue.length - 1] = {
      ...notification,
      event: { ...notification.event, delta: previous.event.delta + notification.event.delta },
    }
    return
  }
  queue.push(notification)
}

function streamKey(notification: RuntimeEventNotification) {
  return `${notification.conversationId}\0${notification.runId}`
}

function orderQueueWithinStreams(queue: RuntimeEventNotification[]) {
  const streams = new Map<string, RuntimeEventNotification[]>()
  const keys = queue.map((notification) => {
    const key = streamKey(notification)
    const events = streams.get(key) ?? []
    events.push(notification)
    streams.set(key, events)
    return key
  })
  for (const events of streams.values()) events.sort((left, right) => left.seq - right.seq)
  const indexes = new Map<string, number>()
  queue.splice(0, queue.length, ...keys.map((key) => {
    const index = indexes.get(key) ?? 0
    indexes.set(key, index + 1)
    return streams.get(key)![index]
  }))
}

function coalesceQueuedDeltas(queue: RuntimeEventNotification[]) {
  if (queue.length < 32) return
  const compacted: RuntimeEventNotification[] = []
  for (const notification of queue) {
    const previous = compacted.at(-1)
    const type = notification.event.type
    if (
      previous
      && pacedEventTypes.has(type)
      && previous.event.type === type
      && previous.runId === notification.runId
      && previous.conversationId === notification.conversationId
      && typeof previous.event.delta === 'string'
      && typeof notification.event.delta === 'string'
    ) {
      compacted[compacted.length - 1] = {
        ...notification,
        event: {
          ...notification.event,
          delta: previous.event.delta + notification.event.delta,
        },
      }
      continue
    }
    compacted.push(notification)
  }
  queue.splice(0, queue.length, ...compacted)
}

function adaptivePacedLimit(queueLength: number, requestedLimit?: number) {
  if (requestedLimit !== undefined) return Math.max(1, requestedLimit)
  if (queueLength > 2_048) return 512
  if (queueLength > 512) return 128
  if (queueLength > 128) return 32
  if (queueLength > 32) return 8
  return 1
}

export function takeRuntimeEventFrame(queue: RuntimeEventNotification[], pacedLimit?: number) {
  if (queue.length === 0) return []
  const limit = adaptivePacedLimit(queue.length, pacedLimit)
  orderQueueWithinStreams(queue)
  coalesceQueuedDeltas(queue)

  let paced = 0
  let count = 0
  while (count < queue.length) {
    const notification = queue[count]
    count += 1
    if (pacedEventTypes.has(notification.event.type)) {
      paced += 1
      if (paced >= limit) break
    }
  }
  return queue.splice(0, count)
}
