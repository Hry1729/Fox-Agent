// Real render test for the package-2-D U1 delivery-acceptance card.
//
// Why this exists: H's D-3 finding was that `AcceptanceIneffective` was serialized
// as an external-tagged Rust enum, so the frontend received `"Absent"` or
// `{"row_not_pending":{…}}` while reading `blocked.kind` — every specific reason
// was silently unreachable and only the generic fallback ever rendered. `tsc`
// could not see it, because the TypeScript type was self-consistent with itself.
//
// This test renders the real component against the real DTO shape the Host emits
// and drives the real buttons, so a contract mismatch fails here rather than in
// the user's face.

// The happy-dom globals are registered by run-component-test.mjs **before** this
// module is imported, because `@testing-library/react`'s `screen` binds
// `document.body` at import time. Registering here instead would be too late.
//
// `assert` is reached through `globalThis.process` rather than a `node:` import,
// because the desktop tsconfig has no `@types/node` and any `node:` specifier
// fails its type-check. The contract check (`tsc --noEmit`) and the run check
// (this test) are separate commands on purpose: the first proves the DTO type is
// consistent, this one proves the DTO actually reaches the DOM.
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type {
  DeliveryAcceptanceDecision,
  DeliveryAcceptanceOutcome,
  DeliveryAcceptanceState,
} from '@/features/conversations/model/types'
import { DeliveryAcceptanceCard } from './DeliveryAcceptanceCard'

type AssertFn = {
  ok: (value: unknown, message?: string) => void
  equal: (actual: unknown, expected: unknown, message?: string) => void
  deepEqual: (actual: unknown, expected: unknown) => void
}

// The one place this test reaches outside the browser-like environment: node's
// assertion library, obtained through the require the runner injects. The
// desktop tsconfig has no `@types/node`, so a `node:` specifier would fail the
// contract check; `globalThis.__nodeRequire` is typed as unknown on purpose.
const nodeRequire = (globalThis as { __nodeRequire?: (id: string) => unknown })
  .__nodeRequire
if (!nodeRequire) {
  throw new Error('run-component-test.mjs must inject require before importing this test')
}
const assert = nodeRequire('node:assert/strict') as AssertFn

function state(overrides: Partial<DeliveryAcceptanceState> = {}): DeliveryAcceptanceState {
  return {
    runId: 'run-1',
    itemKey: 'file:proof.csv',
    verificationClass: 'unconfirmed',
    blocked: null,
    accepted: null,
    rejected: false,
    runTerminal: false,
    ...overrides,
  }
}

const acceptedFact = {
  acceptedBy: 'window:main',
  acceptedAt: 1,
  candidatePath: 'proof.csv',
  candidateSha256: 'a'.repeat(64),
  specHash: 'b'.repeat(64),
}

function outcome(next: DeliveryAcceptanceState): DeliveryAcceptanceOutcome {
  return { runId: next.runId, itemKey: next.itemKey, decision: 'accept', state: next }
}

function renderCard(
  loadState: (runId: string, itemKey: string) => Promise<DeliveryAcceptanceState>,
  resolve: (
    runId: string,
    itemKey: string,
    decision: DeliveryAcceptanceDecision,
  ) => Promise<DeliveryAcceptanceOutcome>,
) {
  return render(
    <DeliveryAcceptanceCard
      runId="run-1"
      itemKey="file:proof.csv"
      description="合并产物"
      loadState={loadState}
      resolve={resolve}
    />,
  )
}

// 1. The verification result and the acceptance are two separate lines.
{
  renderCard(async () => state(), async () => outcome(state()))
  // The card loads its row state asynchronously, so the first render is the
  // loading placeholder. Waiting for the real state is what proves the Host's
  // DTO actually reached the DOM rather than a type-only match.
  await waitFor(() => assert.ok(screen.getByText('未决定')))
  assert.ok(screen.getByText('unconfirmed'))
  cleanup()
}

// 2. An accepted item still reads `unconfirmed`, never `passed`.
{
  renderCard(async () => state({ accepted: acceptedFact }), async () =>
    outcome(state({ accepted: acceptedFact })),
  )
  await waitFor(() => assert.ok(screen.getByText('unconfirmed')))
  assert.ok(screen.getByText(/这不是核验通过/))
  assert.equal(screen.queryByText('passed'), null)
  assert.equal(screen.queryByText('confirmed'), null)
  cleanup()
}

// 3. The specific blocked reason reaches the user from `kind`.
{
  renderCard(
    async () =>
      state({
        blocked: {
          kind: 'candidate_drift',
          detail: '候选文件已偏离接受时核验的字节（期望 e，当前 a），接受失效',
        },
      }),
    async () => outcome(state()),
  )
  await waitFor(() => assert.ok(screen.getByText(/候选文件已偏离接受时核验的字节/)))
  assert.equal(
    (screen.getByRole('button', { name: /接受未核验风险/ }) as HTMLButtonElement).disabled,
    true,
  )
  cleanup()
}

// 4. An unknown kind falls back to the detail line rather than vanishing.
{
  renderCard(
    async () => state({ blocked: { kind: 'some_future_reason', detail: '服务端给出的具体原因' } }),
    async () => outcome(state()),
  )
  await waitFor(() => assert.ok(screen.getByText('服务端给出的具体原因')))
  cleanup()
}

// 5. Accept and reject are separate actions on a fresh row, and the button the
//    user did not press is still there.
{
  const calls: Array<[string, string, DeliveryAcceptanceDecision]> = []
  renderCard(
    async () => state(),
    async (runId, itemKey, decision) => {
      calls.push([runId, itemKey, decision])
      return outcome(state())
    },
  )
  await waitFor(() => assert.ok(screen.getByRole('button', { name: /接受未核验风险/ })))
  fireEvent.click(screen.getByRole('button', { name: /接受未核验风险/ }))
  await waitFor(() => assert.equal(calls.length, 1))
  assert.deepEqual(calls[0], ['run-1', 'file:proof.csv', 'accept'])
  assert.ok(screen.getByRole('button', { name: /判定不可接受/ }))
  cleanup()
}

// 6. Only revoke survives an acceptance.
{
  renderCard(async () => state({ accepted: acceptedFact }), async () =>
    outcome(state({ accepted: acceptedFact })),
  )
  await waitFor(() => assert.ok(screen.getByRole('button', { name: /撤销接受/ })))
  assert.equal(screen.queryByRole('button', { name: /接受未核验风险/ }), null)
  assert.equal(screen.queryByRole('button', { name: /判定不可接受/ }), null)
  cleanup()
}

// 7. The card reloads the row state from the Host's response rather than from a
//    stale local copy, so a terminal the acceptance just committed is visible.
{
  let served = state()
  renderCard(
    async () => served,
    async (_runId, _itemKey, decision) => {
      served = state({ accepted: decision === 'accept' ? acceptedFact : null })
      return { ...outcome(served), decision }
    },
  )
  await waitFor(() => assert.ok(screen.getByRole('button', { name: /接受未核验风险/ })))
  fireEvent.click(screen.getByRole('button', { name: /接受未核验风险/ }))
  await waitFor(() => assert.ok(screen.getByRole('button', { name: /撤销接受/ })))
  cleanup()
}

// 8. A resolve failure is shown as an error and leaves the row state readable,
//    so a failed command never silently looks like success.
{
  renderCard(
    async () => state(),
    async () => {
      throw new Error('run is already terminal (completed)')
    },
  )
  await waitFor(() => assert.ok(screen.getByRole('button', { name: /接受未核验风险/ })))
  fireEvent.click(screen.getByRole('button', { name: /接受未核验风险/ }))
  await waitFor(() => assert.ok(screen.getByText(/run is already terminal/)))
  // The row is still `unconfirmed` and the accept button is back.
  assert.ok(screen.getByText('unconfirmed'))
  assert.ok(screen.getByRole('button', { name: /接受未核验风险/ }))
  cleanup()
}

// 9. R-1: a Run that has ended keeps a reachable revoke entry, and offers no new
//    accept/reject. Before the fix the whole panel vanished with the Run's
//    terminal state, so the user could neither see nor withdraw their acceptance.
{
  renderCard(async () => state({ runTerminal: true, accepted: acceptedFact }), async () =>
    outcome(state({ runTerminal: true, accepted: acceptedFact })),
  )
  await waitFor(() => assert.ok(screen.getByRole('button', { name: /撤销接受/ })))
  assert.equal(screen.queryByRole('button', { name: /接受未核验风险/ }), null)
  assert.equal(screen.queryByRole('button', { name: /判定不可接受/ }), null)
  // The verification result is still stated, and still not "passed".
  assert.ok(screen.getByText('unconfirmed'))
  assert.ok(screen.getByText(/这不是核验通过/))
  cleanup()
}

// 10. R-1: a Run that ended without an acceptance says so plainly instead of
//     hiding the row — an absent button reads as "nothing to do here", which is
//     not the same as "the opportunity has passed".
{
  renderCard(async () => state({ runTerminal: true }), async () =>
    outcome(state({ runTerminal: true })),
  )
  await waitFor(() => assert.ok(screen.getByText('unconfirmed')))
  assert.equal(screen.queryByRole('button', { name: /接受未核验风险/ }), null)
  assert.equal(screen.queryByRole('button', { name: /撤销接受/ }), null)
  assert.ok(screen.getByText(/任务已结束，未对该项作出接受/))
  cleanup()
}

// 11. R-1: while the Run is parked, both decisions are offered — the guard must
//     not over-trigger on a Run that is merely not-yet-terminal.
{
  renderCard(async () => state({ runTerminal: false }), async () => outcome(state()))
  await waitFor(() => assert.ok(screen.getByRole('button', { name: /接受未核验风险/ })))
  assert.ok(screen.getByRole('button', { name: /判定不可接受/ }))
  cleanup()
}

console.log('DELIVERY_ACCEPTANCE_CARD_OK')
