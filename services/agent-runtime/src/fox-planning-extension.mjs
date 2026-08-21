const MAX_CONTEXT_CHARS = 24_000

function compactWorkSnapshot(snapshot) {
  if (!snapshot || typeof snapshot !== 'object') return { goal: null, tasks: [], evidence: [] }
  const details = snapshot.details && typeof snapshot.details === 'object' ? snapshot.details : snapshot
  return {
    goal: details.goal ?? null,
    tasks: Array.isArray(details.tasks) ? details.tasks : [],
    evidence: Array.isArray(details.evidence) ? details.evidence : [],
  }
}

function planningContextMessage(context = {}) {
  const projectRoot = String(context.projectRoot || '').trim()
  const permissionMode = String(context.permissionMode || 'read_only').trim()
  const snapshot = compactWorkSnapshot(context.workSnapshot)
  const serializedSnapshot = JSON.stringify(snapshot, null, 2)
  const boundedSnapshot = serializedSnapshot.length > MAX_CONTEXT_CHARS
    ? `${serializedSnapshot.slice(0, MAX_CONTEXT_CHARS)}\n... [work snapshot truncated by Fox]`
    : serializedSnapshot

  return `[FOX HOST CONTEXT]
This context is supplied by Fox Host and is authoritative for this turn.
- The current conversation identity is Host-owned. Work tools automatically target it; never guess or ask for a conversation ID.
- Authorized project root: ${projectRoot || '(no project authorized)'}
- Project permission mode: ${permissionMode}
- For project tools, use paths relative to the authorized root. The path "." means the project root.
- Goal, Task, Evidence, project authorization, and approval state remain owned by Fox Host. Do not create a second local todo store.
- A proposed Goal is awaiting Host confirmation. Do not create Tasks until its persisted status is active.
- Only report Goal or Task IDs and versions returned by Fox tools.

Current persisted work snapshot:
${boundedSnapshot}`
}

export function createFoxPlanningExtension({ workTools = [], context = {} } = {}) {
  return (pi) => {
    for (const tool of workTools) pi.registerTool(tool)

    pi.on('before_agent_start', async () => ({
      message: {
        customType: 'fox-host-planning-context',
        content: planningContextMessage(context),
        display: false,
        details: { source: 'fox-host', schemaVersion: 1 },
      },
    }))
  }
}

export { compactWorkSnapshot, planningContextMessage }
