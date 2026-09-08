import { expect, test } from 'bun:test'
import { groupRuntimeRecords } from '../src/features/chat/runtime-timeline-performance'

test('retains unchanged run buckets for streaming row memoization', () => {
  const firstEvent = { runId: 'run-a', seq: 1 }
  const otherRunEvent = { runId: 'run-b', seq: 1 }
  const first = groupRuntimeRecords([firstEvent, otherRunEvent], new Map())
  const firstRunBucket = first.get('run-a')
  const otherRunBucket = first.get('run-b')

  const secondEvent = { runId: 'run-a', seq: 2 }
  const second = groupRuntimeRecords([firstEvent, secondEvent, otherRunEvent], first)

  expect(second.get('run-a')).not.toBe(firstRunBucket)
  expect(second.get('run-b')).toBe(otherRunBucket)
  expect(groupRuntimeRecords([firstEvent, secondEvent, otherRunEvent], second).get('run-b')).toBe(otherRunBucket)
})
