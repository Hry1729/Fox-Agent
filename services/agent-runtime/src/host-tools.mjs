import { Type } from '@earendil-works/pi-ai'

async function executeHostTool(toolCallId, tool, input, requestHost, signal) {
  const response = await requestHost('tool.execute', { toolCallId, tool, input }, signal)
  const payload = response?.payload ?? {}
  if (payload.isError || !payload.result) {
    throw new Error(payload.error || `Fox host tool ${tool} failed.`)
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
      description: 'Create or replace a UTF-8 text file inside the authorized project. Fox may require user approval before the host writes it.',
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
      description: 'Replace an exact text section in a UTF-8 file inside the authorized project. Fox shows a diff and enforces the project permission mode.',
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
      description: 'Run a non-interactive command with a working directory inside the authorized project. Fox may require explicit approval.',
      parameters: Type.Object({
        command: Type.String(),
        cwd: Type.Optional(Type.String()),
        timeoutSeconds: Type.Optional(Type.Number()),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'run_command', params, requestHost, signal),
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
      description: 'Call one validated MCP tool through Fox. Every call requires explicit user approval.',
      parameters: Type.Object({
        serverId: Type.String(),
        tool: Type.String(),
        arguments: Type.Optional(Type.Record(Type.String(), Type.Unknown())),
      }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'call_mcp_tool', params, requestHost, signal),
    },
  ]
}
