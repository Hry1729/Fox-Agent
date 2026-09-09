import { describe, expect, test } from 'bun:test'
import { parseQueryArguments, reconciliationReady, type ReconciliationView } from '../src/features/conversations/model/reconciliation'

const fixture = (): ReconciliationView => ({ runId: 'old', revision: 2, recoveryRunId: null, canResume: true, items: [
  { effectKey: 'effect', tool: 'write_file', input: {}, evidence: null, decision: 'executed', note: 'External receipt 123 verified by user' },
] })
describe('Kernel reconciliation controls', () => {
  test('observed evidence alone or unresolved decisions never enable recovery', () => {
    const view = fixture()
    expect(reconciliationReady(view)).toBe(true)
    view.items[0].decision = 'unresolved'
    expect(reconciliationReady(view)).toBe(false)
    view.items[0].decision = null
    view.items[0].evidence = { matchesSubmittedContent: true }
    expect(reconciliationReady(view)).toBe(false)
    view.items[0].decision = 'executed'
    view.items[0].note = ''
    expect(reconciliationReady(view)).toBe(false)
  })
  test('server rejection and existing recovery link disable duplicate recovery', () => {
    const view = fixture()
    view.canResume = false
    expect(reconciliationReady(view)).toBe(false)
    view.canResume = true; view.recoveryRunId = 'new'
    expect(reconciliationReady(view)).toBe(false)
  })
  test('connector query input is bounded and must be an object', () => {
    expect(parseQueryArguments('{"receiptId":"test-123"}')).toEqual({ receiptId: 'test-123' })
    for (const text of ['null', '[]', '42', 'broken', JSON.stringify({ large: 'x'.repeat(16384) })]) expect(() => parseQueryArguments(text)).toThrow()
  })
})
