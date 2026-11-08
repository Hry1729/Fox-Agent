import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { projectKernelGroups, projectRuntimeGroups } from '../src/features/chat/runtime-process-groups'
import { splitAssistantContent } from '../src/features/chat/timeline/fox-assistant-content'
import { answerDeltaFingerprint } from '../src/features/conversations/model/runtime-delta-fingerprint'
import type { ConversationMessage, RunEventRecord } from '../src/features/conversations/model/types'

// Shared with the Rust projection test in
// apps/desktop/src-tauri/src/database/repositories/contact_bubbles.rs:
// both sides must produce the same bubble ids and visible text, one by one.
interface FixtureCase {
  name: string
  mode: 'runtime' | 'kernel' | 'kernelFallback'
  runId: string
  content: string
  events: { seq: number; eventType: string; payload: Record<string, unknown>; createdAt: number }[]
  kernelCheckpoints?: { checkpointSeq: number; content: string; createdAt: number }[]
  expected: { id: string; text: string }[]
}

const fixture = JSON.parse(readFileSync(new URL('./fixtures/contact-bubbles.json', import.meta.url), 'utf8')) as { cases: FixtureCase[] }

function runEvents(item: FixtureCase): RunEventRecord[] {
  return item.events.map(entry => {
    const event: Record<string, unknown> = { type: entry.eventType, ...entry.payload }
    if (entry.eventType === 'message.delta' && typeof entry.payload.delta === 'string') {
      const delta = entry.payload.delta
      event.deltaLength = delta.length
      event.deltaFingerprint = answerDeltaFingerprint(delta)
    }
    return { runId: item.runId, seq: entry.seq, eventType: entry.eventType, event: event as RunEventRecord['event'], createdAt: entry.createdAt }
  })
}

function kernelMessages(item: FixtureCase): ConversationMessage[] {
  return (item.kernelCheckpoints ?? []).map(checkpoint => ({
    id: `kernel-message:${item.runId}:${checkpoint.checkpointSeq}`,
    runId: item.runId, conversationId: 'chat', role: 'assistant', kind: 'text',
    content: checkpoint.content, status: 'completed', ordinal: checkpoint.checkpointSeq,
    createdAt: checkpoint.createdAt, updatedAt: checkpoint.createdAt,
  }))
}

function project(item: FixtureCase): { id: string; text: string }[] {
  const events = runEvents(item)
  const groups = item.mode === 'kernel'
    ? projectKernelGroups(item.runId, kernelMessages(item), events)
    : projectRuntimeGroups(item.runId, item.content, events)
  expect(groups, item.name).not.toBeNull()
  return (groups ?? []).flatMap(group => {
    if (group.kind !== 'response') return []
    const text = splitAssistantContent(group.text).answer
    return text.trim() ? [{ id: group.key, text }] : []
  })
}

describe('contact-bubbles shared fixture (TS side)', () => {
  for (const item of fixture.cases) {
    test(item.name, () => {
      expect(project(item)).toEqual(item.expected)
    })
  }
})