import { Type } from '@earendil-works/pi-ai'
import { executeReadOnlyTool } from './read-only-tool-executors.mjs'

async function approvedArgs(tool, params, requestPreflight, signal) {
  const response = await requestPreflight(tool, params, signal)
  if (response.decision !== 'allow' || !response.input) {
    throw new Error(response.message || `Tool ${tool} was blocked by Fox.`)
  }
  return response.input
}

export function createReadOnlyTools(requestPreflight) {
  return [
    {
      name: 'read',
      label: 'Read file',
      description: 'Read a UTF-8 text file inside the authorized project folder.',
      parameters: Type.Object({ path: Type.String(), offset: Type.Optional(Type.Number()), limit: Type.Optional(Type.Number()) }),
      execute: async (_toolCallId, params, signal) => {
        const input = await approvedArgs('read', params, requestPreflight, signal)
        return executeReadOnlyTool('read', input, { signal })
      },
    },
    {
      name: 'ls',
      label: 'List directory',
      description: 'List direct children of a directory inside the authorized project folder.',
      parameters: Type.Object({ path: Type.String() }),
      execute: async (_toolCallId, params, signal) => {
        const input = await approvedArgs('ls', params, requestPreflight, signal)
        return executeReadOnlyTool('ls', input, { signal })
      },
    },
    {
      name: 'find',
      label: 'Find files',
      description: 'Find file and directory names below a project path.',
      parameters: Type.Object({ path: Type.String(), pattern: Type.String() }),
      execute: async (_toolCallId, params, signal) => {
        const input = await approvedArgs('find', params, requestPreflight, signal)
        return executeReadOnlyTool('find', input, { signal })
      },
    },
    {
      name: 'grep',
      label: 'Search files',
      description: 'Search text files below a project path for a literal string.',
      parameters: Type.Object({ path: Type.String(), pattern: Type.String() }),
      execute: async (_toolCallId, params, signal) => {
        const input = await approvedArgs('grep', params, requestPreflight, signal)
        return executeReadOnlyTool('grep', input, { signal })
      },
    },
  ]
}
