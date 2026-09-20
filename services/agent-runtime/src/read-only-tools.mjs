import { Type } from 'typebox'
import { executeReadOnlyTool } from './read-only-tool-executors.mjs'
import { defineFoxTools } from './tool-adapter.mjs'

async function approvedArgs(toolCallId, tool, params, requestPreflight, signal) {
  const input = tool !== 'read' && !String(params?.path || '').trim()
    ? { ...params, path: '.' }
    : params
  const response = await requestPreflight(toolCallId, tool, input, signal)
  if (response.decision !== 'allow' || !response.input) {
    throw new Error(response.message || `Tool ${tool} was blocked by Fox.`)
  }
  return response
}

export function createReadOnlyTools(requestPreflight, { limits, executeHost } = {}) {
  const execute = async (toolCallId, tool, params, signal) => {
    const approved = await approvedArgs(toolCallId, tool, params, requestPreflight, signal)
    const route = approved.executionRoute ?? 'runtime'
    if (route === 'rust') {
      if (typeof executeHost !== 'function' || !approved.permissionSnapshotId) throw new Error('Frozen Rust reader route is unavailable.')
      const response = await executeHost('tool.readonly_execute', {
        toolCallId, tool, input: approved.input, originalInput: params ?? {}, permissionSnapshotId: approved.permissionSnapshotId,
      }, signal)
      if (response?.type !== 'tool.execute_completed' || response.payload?.isError
          || !Array.isArray(response.payload?.result?.content)) {
        throw new Error(response?.payload?.error || 'Rust resource gateway rejected the operation.')
      }
      return response.payload.result
    }
    if (route !== 'runtime') throw new Error(`Unknown frozen read-only execution route: ${route}`)
    return executeReadOnlyTool(tool, approved.input, {
      signal,
      limits,
      authorizationScope: approved.permissionSnapshotId ?? '',
    })
  }
  return defineFoxTools([
    {
      name: 'read',
      label: 'Read file',
            description: 'Read a UTF-8 text file inside the authorized project folder. Use offset/limit (UTF-16 code units, never split surrogate pairs; nextOffset advances by actually returned units) or startLine/lineCount (1-based lines, mutually exclusive with offset/limit). The Rust reader also extracts DOCX, XLSX and PPTX text; spreadsheet formulas use saved results, not recalculation. Legacy DOC/XLS/PPT require conversion.',
      parameters: Type.Object({ path: Type.String(), offset: Type.Optional(Type.Number()), limit: Type.Optional(Type.Number()), startLine: Type.Optional(Type.Number()), lineCount: Type.Optional(Type.Number()) }),
      execute: async (toolCallId, params, signal) => {
        return execute(toolCallId, 'read', params, signal)
      },
    },
    {
      name: 'ls',
      label: 'List directory',
      description: 'List direct children of a directory inside the authorized project folder. Omit path to list the project root.',
      parameters: Type.Object({ path: Type.Optional(Type.String()) }),
      execute: async (toolCallId, params, signal) => {
        return execute(toolCallId, 'ls', params, signal)
      },
    },
    {
      name: 'find',
      label: 'Find files',
            description: 'Find file and directory names below a project path. Omit path to search from the project root. Supports caseSensitive, regex and glob options. Respects dependency/build ignore rules. Pass cursor (from a previous nextCursor) with the SAME pattern/options/scope to continue; null nextCursor means the scope is exhausted.',
      parameters: Type.Object({ path: Type.Optional(Type.String()), pattern: Type.String(), caseSensitive: Type.Optional(Type.Boolean()), regex: Type.Optional(Type.Boolean()), glob: Type.Optional(Type.String()), cursor: Type.Optional(Type.String()) }),
      execute: async (toolCallId, params, signal) => {
        return execute(toolCallId, 'find', params, signal)
      },
    },
    {
      name: 'grep',
      label: 'Search files',
      description: 'Search text files below a project path. Omit path to search from the project root. Supports caseSensitive, regex and glob options. Respects dependency/build ignore rules. Pass cursor (from a previous nextCursor) with the SAME pattern/options/scope to continue; null nextCursor means the scope is exhausted.',
      parameters: Type.Object({ path: Type.Optional(Type.String()), pattern: Type.String(), caseSensitive: Type.Optional(Type.Boolean()), regex: Type.Optional(Type.Boolean()), glob: Type.Optional(Type.String()), cursor: Type.Optional(Type.String()) }),
      execute: async (toolCallId, params, signal) => {
        return execute(toolCallId, 'grep', params, signal)
      },
    },
  ], { source: 'fox-read-only', execution: 'runtime', trusted: true })
}
