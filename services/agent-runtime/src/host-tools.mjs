import { Type } from '@earendil-works/pi-ai'

async function executeHostTool(toolCallId, tool, input, requestHost, signal) {
  const response = await requestHost('tool.execute', { toolCallId, tool, input }, signal)
  const payload = response?.payload ?? {}
  if (payload.isError || !payload.result) {
    const baseMessage = payload.error || `Fox host tool ${tool} failed.`
    const serializedDetails = payload.errorDetails && typeof payload.errorDetails === 'object'
      ? JSON.stringify(payload.errorDetails)
      : ''
    const error = new Error(serializedDetails
      ? `${baseMessage}\nFox error details: ${serializedDetails}`
      : baseMessage)
    if (payload.errorDetails && typeof payload.errorDetails === 'object') {
      error.details = payload.errorDetails
    }
    throw error
  }
  return payload.result
}

export function createHostTools(requestHost) {
  return [
    {
      name: 'read_attachment',
      label: 'Read attachment',
      description: 'Read a UTF-8 text or DOCX attachment from the current conversation by its Fox attachment ID. Other binary formats and extracted text larger than 1 MiB are rejected.',
      parameters: Type.Object({ attachmentId: Type.String() }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'read_attachment', params, requestHost, signal),
    },
    {
      name: 'write_file',
      label: 'Write file',
      description: 'Create or replace a UTF-8 text file inside the authorized project. Call this tool for a real file write; describing a write in text does nothing. Fox may require user approval before the host writes it.',
      parameters: Type.Object({
        path: Type.String(),
        content: Type.String(),
        createDirectories: Type.Optional(Type.Boolean()),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'write_file', params, requestHost, signal),
    },
    {
      name: 'edit_file',
      label: 'Edit file',
      description: 'Replace an exact text section in a UTF-8 file inside the authorized project. Call this tool for a real edit; Fox shows a diff and enforces the project permission mode.',
      parameters: Type.Object({
        path: Type.String(),
        oldText: Type.String(),
        newText: Type.String(),
        replaceAll: Type.Optional(Type.Boolean()),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'edit_file', params, requestHost, signal),
    },
    {
      name: 'run_command',
      label: 'Run command',
      description: 'Run a real non-interactive command with a working directory inside the authorized project. Every call requires explicit approval. On Windows, wrap PowerShell cmdlets with powershell -NoProfile -Command "...".',
      parameters: Type.Object({
        command: Type.String(),
        cwd: Type.Optional(Type.String()),
        timeoutSeconds: Type.Optional(Type.Number()),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'run_command', params, requestHost, signal),
    },
    {
      name: 'work_snapshot_get',
      label: 'Get work snapshot',
      description: 'Load the current goal, tasks, and evidence for the Host-owned current conversation. The conversation identity is injected by Fox.',
      parameters: Type.Object({}),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'work_snapshot_get', params, requestHost, signal),
    },
    {
      name: 'goal_propose',
      label: 'Propose goal',
      description: 'Propose one substantive goal only when the latest user message explicitly asks Fox to create or track a goal or execution plan. Never infer goal intent from pasted content, Markdown headings, educational text, examples, or incidental planning words. If Fox already created an empty host placeholder, this call refines that same goal before tasks exist.',
      parameters: Type.Object({
        title: Type.String(),
        objective: Type.String(),
        acceptanceSummary: Type.Optional(Type.String()),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'goal_propose', params, requestHost, signal),
    },
    {
      name: 'goal_complete',
      label: 'Complete goal',
      description: 'Ask Fox Host to complete an active goal after every required task is completed or skipped. Validate the evidence for every completed task first. Fox validates conversation ownership, task state, valid evidence, and the optimistic goal version before persisting completion.',
      parameters: Type.Object({
        goalId: Type.String(),
        expectedVersion: Type.Integer({ minimum: 1 }),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'goal_complete', params, requestHost, signal),
    },
    {
      name: 'task_create_many',
      label: 'Create work tasks',
      description: 'Atomically create ordered tasks under an active goal owned by this conversation. A proposed goal must first be confirmed and activated by Fox Host.',
      parameters: Type.Object({
        goalId: Type.String(),
        tasks: Type.Array(Type.Object({
          title: Type.String(),
          detail: Type.Optional(Type.String()),
          ordinal: Type.Integer({ minimum: 0 }),
        }), { minItems: 1 }),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'task_create_many', params, requestHost, signal),
    },
    {
      name: 'task_update',
      label: 'Update work task',
      description: 'Apply a legal task state transition through Fox using optimistic concurrency.',
      parameters: Type.Object({
        taskId: Type.String(),
        status: Type.Union([
          Type.Literal('queued'),
          Type.Literal('in_progress'),
          Type.Literal('completed'),
          Type.Literal('blocked'),
          Type.Literal('interrupted'),
          Type.Literal('skipped'),
        ]),
        expectedVersion: Type.Integer({ minimum: 1 }),
        blockedReason: Type.Optional(Type.String()),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'task_update', params, requestHost, signal),
    },
    {
      name: 'task_evidence_add',
      label: 'Add task evidence',
      description: 'Attach validated evidence to a task in this conversation.',
      parameters: Type.Object({
        taskId: Type.String(),
        evidenceType: Type.Union([
          Type.Literal('tool_call'), Type.Literal('trace_span'), Type.Literal('test_result'),
          Type.Literal('file_diff'), Type.Literal('artifact'), Type.Literal('user_confirmation'),
          Type.Literal('external_reference'),
        ]),
        refKind: Type.Union([
          Type.Literal('tool_call'), Type.Literal('artifact'), Type.Literal('run_event'),
          Type.Literal('message'), Type.Literal('source'),
        ]),
        refId: Type.String(),
        summary: Type.String(),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'task_evidence_add', params, requestHost, signal),
    },
    {
      name: 'task_evidence_validate',
      label: 'Validate task evidence',
      description: 'Revalidate one evidence reference and return its current validity status.',
      parameters: Type.Object({ evidenceId: Type.String() }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'task_evidence_validate', params, requestHost, signal),
    },
    {
      name: 'plan_revision_create',
      label: 'Create plan revision',
      description: 'Persist a new approved PlanRevision for an active goal. Use this when the execution plan changes, and include the complete ordered task plan rather than only the delta.',
      parameters: Type.Object({
        goalId: Type.String(),
        title: Type.String(),
        summary: Type.String(),
        tasks: Type.Array(Type.Object({ title: Type.String(), detail: Type.Optional(Type.String()), ordinal: Type.Integer({ minimum: 0 }) })),
      }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'plan_revision_create', params, requestHost, signal),
    },
    {
      name: 'review_finding_add',
      label: 'Add review finding',
      description: 'Persist an independent review result. Record real issues as open findings. When review finds no blocker, add one resolved info finding that states the review scope and checks performed.',
      parameters: Type.Object({
        goalId: Type.String(), taskId: Type.Optional(Type.String()), planRevisionId: Type.Optional(Type.String()),
        severity: Type.Union([Type.Literal('critical'), Type.Literal('high'), Type.Literal('medium'), Type.Literal('low'), Type.Literal('info')]),
        category: Type.String(), title: Type.String(), detail: Type.String(),
        status: Type.Optional(Type.Union([Type.Literal('open'), Type.Literal('resolved'), Type.Literal('waived')])),
        reviewer: Type.String(),
      }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'review_finding_add', params, requestHost, signal),
    },
    {
      name: 'review_finding_resolve',
      label: 'Resolve review finding',
      description: 'Resolve or explicitly waive a persisted review finding after the issue has been addressed or accepted.',
      parameters: Type.Object({ findingId: Type.String(), status: Type.Union([Type.Literal('resolved'), Type.Literal('waived')]) }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'review_finding_resolve', params, requestHost, signal),
    },
    {
      name: 'acceptance_submit',
      label: 'Submit final acceptance',
      description: 'Submit final A1 acceptance. Fox Host requires an approved PlanRevision, an independent review record, no open blocking findings, terminal tasks, valid evidence, and the current goal version before completing the goal.',
      parameters: Type.Object({ goalId: Type.String(), expectedVersion: Type.Integer({ minimum: 1 }), summary: Type.String(), reviewer: Type.String() }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'acceptance_submit', params, requestHost, signal),
    },
  ]
}

export function createKnowledgeTools(requestHost) {
  return [
    {
      name: 'list_knowledge_bases',
      label: 'List knowledge bases',
      description: 'List Yuxi knowledge bases explicitly enabled for the current Fox conversation.',
      parameters: Type.Object({}),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'list_knowledge_bases', params, requestHost, signal),
    },
    {
      name: 'search_knowledge',
      label: 'Search knowledge',
      description: 'Search one Yuxi knowledge base enabled for the current conversation and return source metadata.',
      parameters: Type.Object({ knowledgeBaseId: Type.String(), query: Type.String() }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'search_knowledge', params, requestHost, signal),
    },
    {
      name: 'read_knowledge_document',
      label: 'Read knowledge document',
      description: 'Read an already parsed document from a Yuxi knowledge base enabled for the conversation.',
      parameters: Type.Object({ knowledgeBaseId: Type.String(), documentId: Type.String() }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'read_knowledge_document', params, requestHost, signal),
    },
    {
      name: 'query_knowledge_graph',
      label: 'Query knowledge graph',
      description: 'Query a bounded Yuxi knowledge graph subgraph from a knowledge base enabled for the current conversation.',
      parameters: Type.Object({
        knowledgeBaseId: Type.String(),
        keyword: Type.Optional(Type.String()),
        maxDepth: Type.Optional(Type.Number()),
        maxNodes: Type.Optional(Type.Number()),
      }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'query_knowledge_graph', params, requestHost, signal),
    },
  ]
}

export function createMcpTools(requestHost) {
  return [
    {
      name: 'list_mcp_tools',
      label: 'List MCP tools',
      description: 'List enabled MCP servers and their validated tool catalog. Use this before calling an MCP tool.',
      parameters: Type.Object({ query: Type.Optional(Type.String()) }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'list_mcp_tools', params, requestHost, signal),
    },
    {
      name: 'call_mcp_tool',
      label: 'Call MCP tool',
      description: 'Call one validated MCP tool through Fox. Use this for a real MCP operation after list_mcp_tools; every call requires explicit user approval.',
      parameters: Type.Object({
        serverId: Type.String(),
        tool: Type.String(),
        arguments: Type.Optional(Type.Record(Type.String(), Type.Unknown())),
      }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'call_mcp_tool', params, requestHost, signal),
    },
  ]
}
