import { describe, expect, test } from 'bun:test'
import { createOperationState, isPartialOperation, reduceOperationEvent, reduceOperationEvents } from '../src/features/local-knowledge/operation-reducer'
import type { KnowledgeReference, OperationEvent } from '../src/features/local-knowledge/model'

function event(sequence: number, status: OperationEvent['status'], overrides: Partial<OperationEvent> = {}): OperationEvent {
  return {
    schemaVersion: 1,
    operationId: 'operation-1',
    sequence,
    domain: 'knowledge',
    operation: 'documents_import',
    status,
    stage: 'embedding',
    progress: sequence * 10,
    occurredAt: `2026-08-22T00:00:0${sequence}.000Z`,
    ...overrides,
  }
}

describe('local knowledge operation reducer', () => {
  test('deduplicates repeated and out-of-order sequence values', () => {
    const state = reduceOperationEvents(createOperationState('operation-1'), [
      event(1, 'running', { progress: 10 }),
      event(3, 'completed', { progress: 100 }),
      event(2, 'running', { progress: 20 }),
      event(3, 'completed', { progress: 100 }),
    ])

    expect(state.lastSequence).toBe(3)
    expect(state.status).toBe('completed')
    expect(state.progress).toBe(100)
    expect(state.lastEvent?.sequence).toBe(3)
  })

  test('preserves partial outcome on a completed operation', () => {
    const state = reduceOperationEvent(createOperationState('operation-1'), event(4, 'completed', {
      progress: 100,
      stage: 'committing',
      data: { completedItems: 3, failedItems: 1, totalItems: 4, outcome: 'partial' },
    }))

    expect(isPartialOperation(state)).toBe(true)
    expect(state.outcome).toBe('partial')
    expect(state.data).toMatchObject({ completedItems: 3, failedItems: 1, totalItems: 4 })
  })

  test('does not reduce an event belonging to another operation', () => {
    const state = reduceOperationEvent(createOperationState('operation-1'), event(1, 'running', { operationId: 'operation-2' }))
    expect(state.lastSequence).toBe(0)
    expect(state.status).toBe('idle')
  })

  test('keeps local and remote knowledge references explicitly distinguishable', () => {
    const local: KnowledgeReference = { source: 'local', id: 'kb-local', revision: 'generation:3' }
    const remote: KnowledgeReference = { source: 'remote', connectionId: 'connection-1', id: 'kb-remote' }

    expect(local.source).toBe('local')
    expect(remote.source).toBe('remote')
    expect('connectionId' in local).toBe(false)
    expect(remote.connectionId).toBe('connection-1')
  })
})
