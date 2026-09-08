import { RUNTIME_TOOL_CATALOG } from './runtime-contract.mjs'

const PROJECT_TOOL_NAMES = new Set(RUNTIME_TOOL_CATALOG
  .filter(({ category }) => ['project-read', 'project-write', 'process'].includes(category))
  .map(({ name }) => name))

export function selectToolsForProjectContext(tools, projectContext) {
  const hasProject = typeof projectContext?.projectRoot === 'string'
    && projectContext.projectRoot.trim().length > 0
  return {
    tools: tools.filter(({ name }) => hasProject || !PROJECT_TOOL_NAMES.has(name)),
    excludedTools: hasProject ? [] : tools
      .filter(({ name }) => PROJECT_TOOL_NAMES.has(name))
      .map(({ name }) => ({ name, reason: 'project_unavailable' })),
  }
}

function normalizeDeclaredToolNames(agentPackage) {
  const configured = agentPackage?.packageManifest?.allowedTools
  if (!Array.isArray(configured)) return null
  return [...new Set(configured
    .filter((name) => typeof name === 'string')
    .map((name) => name.trim())
    .filter(Boolean))]
    .sort()
}

function isAllowed(toolName, declaredToolNames) {
  return declaredToolNames === null || declaredToolNames.includes(toolName)
}

export function diagnoseToolsForAgentContext(tools, assistantPackage, expertPackage) {
  const catalog = Array.isArray(tools) ? tools : []
  const assistantDeclaredToolNames = normalizeDeclaredToolNames(assistantPackage)
  const expertDeclaredToolNames = normalizeDeclaredToolNames(expertPackage)
  const registeredToolNames = new Set(catalog.map((tool) => tool.name))
  const effectiveTools = []
  const excludedTools = []

  for (const tool of catalog) {
    const reasons = []
    if (!isAllowed(tool.name, assistantDeclaredToolNames)) reasons.push('assistant')
    if (!isAllowed(tool.name, expertDeclaredToolNames)) reasons.push('expert')
    if (reasons.length === 0) effectiveTools.push(tool)
    else excludedTools.push({ name: tool.name, reason: reasons[0], reasons })
  }

  const unknownDeclarations = new Map()
  for (const [declaredBy, names] of [
    ['assistant', assistantDeclaredToolNames],
    ['expert', expertDeclaredToolNames],
  ]) {
    for (const name of names ?? []) {
      if (registeredToolNames.has(name)) continue
      const sources = unknownDeclarations.get(name) ?? []
      sources.push(declaredBy)
      unknownDeclarations.set(name, sources)
    }
  }
  for (const name of [...unknownDeclarations.keys()].sort()) {
    excludedTools.push({
      name,
      reason: 'unregistered',
      declaredBy: unknownDeclarations.get(name),
    })
  }

  return {
    tools: effectiveTools,
    assistantDeclaredToolNames,
    expertDeclaredToolNames,
    effectiveToolNames: effectiveTools.map((tool) => tool.name),
    excludedTools,
  }
}

export function filterToolsForAgentContext(tools, assistantPackage, expertPackage) {
  const diagnostics = diagnoseToolsForAgentContext(tools, assistantPackage, expertPackage)
  if (diagnostics.assistantDeclaredToolNames === null && diagnostics.expertDeclaredToolNames === null) {
    return Array.isArray(tools) ? tools : []
  }
  return diagnostics.tools
}

export function filterToolsForExpert(tools, expertPackage) {
  return filterToolsForAgentContext(tools, null, expertPackage)
}
