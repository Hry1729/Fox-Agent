import { expect, test } from 'bun:test'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { EMPTY_CONVERSATION_RUN_OVERLAY } from './conversation-run-indicator'
import { COMPLETION_HISTORY_INITIALIZED_KEY, useConversationCompletionNotices } from './use-conversation-completion-notices'

const summary = (id, runId, extra = {}) => ({ id, title: id, agentId: 'fox-general', agentName: 'Fox', projectId: null, projectRoot: null, status: 'active', createdAt: 1, updatedAt: 1, lastMessageAt: 1, lastRunId: runId, lastRunStatus: 'completed', ...extra })

function harness(values = new Map()) {
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) }
  const root = createRoot(document.createElement('div'))
  let notices
  function Scene({ conversations = [], snapshotAt = 0, overlay = EMPTY_CONVERSATION_RUN_OVERLAY }) {
    notices = useConversationCompletionNotices(conversations, overlay, snapshotAt, storage, true)
    return null
  }
  return {
    values,
    get notices() { return notices },
    render: props => act(async () => root.render(createElement(Scene, props))),
    unmount: () => act(async () => root.unmount()),
  }
}

test('history waits for the first list and preserves running, approval, reply and live completions', async () => {
  const view = harness()
  try {
    await view.render({})
    expect(view.values.has(COMPLETION_HISTORY_INITIALIZED_KEY)).toBe(false)
    const future = Date.now() + 1000
    await view.render({ snapshotAt: future, conversations: [
      summary('old', 'old-run'),
      summary('running', 'run', { activeRunId: 'run', activeRunStatus: 'running' }),
      summary('approval', 'approval-run', { activeRunId: 'approval-run', awaitingApproval: true }),
      summary('reply', 'reply-run', { awaitingReply: true }),
      summary('failed', 'failed-run', { lastRunStatus: 'failed' }),
      summary('fresh', 'fresh-run', { updatedAt: future }),
      summary('live', 'live-run'),
    ], overlay: new Map([['live', { indicator: 'completed', runId: 'live-run', updatedAt: future + 1 }]]) })
    expect(view.notices.runIndicators.has('old')).toBe(false)
    expect(view.notices.runIndicators.get('running')).toBe('running')
    expect(view.notices.runIndicators.get('approval')).toBe('approval')
    expect(view.notices.runIndicators.get('reply')).toBe('reply')
    expect(view.notices.runIndicators.has('failed')).toBe(false)
    expect(view.notices.runIndicators.get('fresh')).toBe('completed')
    expect(view.notices.runIndicators.get('live')).toBe('completed')
    expect(view.values.get(COMPLETION_HISTORY_INITIALIZED_KEY)).toBe('1')
  } finally { await view.unmount() }
})

test('initial history is acknowledged once; a later run stays unread across remounts until viewed', async () => {
  const values = new Map()
  const initial = harness(values)
  try {
    await initial.render({ snapshotAt: 10, conversations: [summary('a', 'run-1')] })
    expect(initial.notices.runIndicators.has('a')).toBe(false)
    await initial.render({ snapshotAt: 20, conversations: [summary('a', 'run-2')] })
    expect(initial.notices.runIndicators.get('a')).toBe('completed')
  } finally { await initial.unmount() }
  const restored = harness(values)
  try {
    await restored.render({ snapshotAt: 30, conversations: [summary('a', 'run-2')] })
    expect(restored.notices.runIndicators.get('a')).toBe('completed')
    await act(async () => restored.notices.acknowledgeCompletion('a'))
    expect(restored.notices.runIndicators.has('a')).toBe(false)
  } finally { await restored.unmount() }
})

test('an authoritative empty first list does not make the first new result read', async () => {
  const view = harness()
  try {
    await view.render({ snapshotAt: 10 })
    expect(view.values.get(COMPLETION_HISTORY_INITIALIZED_KEY)).toBe('1')
    await view.render({ snapshotAt: 20, conversations: [summary('new', 'new-run')] })
    expect(view.notices.runIndicators.get('new')).toBe('completed')
  } finally { await view.unmount() }
})
