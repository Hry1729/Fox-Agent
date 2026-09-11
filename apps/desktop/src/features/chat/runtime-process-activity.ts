import type { RunEventRecord } from '@/features/conversations/model/types'

const terminalTypes = new Set(['run.completed', 'run.cancelled', 'run.failed', 'run.interrupted'])

/** Activity follows events; finishing a tool must not restart earlier reasoning. */
export function runtimeProcessActivity(events: readonly RunEventRecord[], running: boolean, answerStarted = false) {
  const activeToolIds = new Set<string>()
  const terminalEvent = [...events].reverse().find(item => terminalTypes.has(item.eventType))
  const active = running && !terminalEvent
  let reasoning = false
  if (active) for (const item of events) {
    const toolId = typeof item.event.toolCallId === 'string' ? item.event.toolCallId : `tool-${item.seq}`
    if (item.eventType === 'tool.started') {
      activeToolIds.add(toolId)
      reasoning = false
    } else if (['tool.completed', 'tool.failed', 'tool.cancelled'].includes(item.eventType)) {
      activeToolIds.delete(toolId)
      reasoning = false
    } else if (item.eventType === 'reasoning.delta' && typeof item.event.delta === 'string' && item.event.delta.trim()) {
      reasoning = item.event.source !== 'yuxi-history' || !answerStarted
    } else if (item.eventType === 'message.completed' || item.eventType === 'message.delta' && typeof item.event.delta === 'string' && item.event.delta.trim()) {
      reasoning = false
    }
  }
  return { running: active, terminalEvent, activeToolIds, reasoning: active && reasoning && activeToolIds.size === 0 }
}
