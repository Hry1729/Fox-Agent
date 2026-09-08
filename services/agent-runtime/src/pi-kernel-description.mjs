// Pure preparation only. These builders produce descriptions; their executors
// are discarded and never installed in a model session or invoked here.
import { createReadOnlyTools } from './read-only-tools.mjs'
import { createGraphReadonlyTools } from './graph-readonly-tools.mjs'
import { createHostTools, createKnowledgeTools, createMcpTools } from './host-tools.mjs'
import { diagnoseToolsForAgentContext, selectToolsForProjectContext } from './expert-package.mjs'
import { resolveExecutionProfile, executionProfileSnapshot, executionProfilePrompt, applyExecutionProfileToTools } from './execution-profile.mjs'
import { resolveModelProfile, modelProfilePrompt } from './model-profile.mjs'
import { composeRuntimePrompt } from './runtime-instructions.mjs'

export function describeKernelRun(request) {
  const { executionProfileId, modelService, prompt, supportedTools } = request.payload ?? {}
  if (!prompt || typeof prompt !== 'object' || !Array.isArray(supportedTools)
      || !supportedTools.every(name => typeof name === 'string') || supportedTools.length > 128) {
    throw new Error('Invalid Host preparation inputs')
  }
  const profile = resolveExecutionProfile(executionProfileId)
  const modelProfile = resolveModelProfile(modelService)
  const context = {
    projectRoot: prompt.projectContext?.projectRoot,
    permissionMode: prompt.projectContext?.permissionMode,
    workSnapshot: prompt.workSnapshot,
    memoryContext: prompt.memoryContext,
    assistantPackage: prompt.assistantPackage,
    expertPackage: prompt.expertPackage,
    expertBinding: prompt.expertBinding,
    skillPrompt: prompt.skillPrompt,
    runContext: prompt.runContext,
    conversationId: request.conversationId,
    model: modelService.modelId,
    executionProfile: executionProfileSnapshot(profile),
  }
  const unavailable = () => { throw new Error('Description mode cannot execute resources') }
  const catalog = [
    ...createReadOnlyTools(unavailable, { executeHost: unavailable }),
    ...createGraphReadonlyTools({ profile, preflight: unavailable, executeHost: unavailable, context }),
    ...createHostTools(unavailable), ...createKnowledgeTools(unavailable), ...createMcpTools(unavailable),
  ]
  const scoped = diagnoseToolsForAgentContext(catalog, prompt.assistantPackage, prompt.expertPackage)
  const profiled = applyExecutionProfileToTools(scoped.tools, profile)
  const selected = selectToolsForProjectContext(profiled.tools, prompt.projectContext)
  const supported = new Set(supportedTools)
  const proposalTools = selected.tools.filter(tool => supported.has(tool.name))
    .map(({ name, description, parameters }) => ({ name, description, parameters }))
  const composed = composeRuntimePrompt({
    systemPrompt: prompt.systemPrompt,
    modelInstructions: [modelProfilePrompt(modelProfile), executionProfilePrompt(profile)].filter(Boolean).join('\n\n'),
    approvalDemo: false,
    budget: prompt.promptBudget,
    context,
    turn: { cwd: context.projectRoot || '.', recovery: null, plannerPlan: null },
  })
  return { schemaVersion: 1, executionProfileId: profile.id, systemPrompt: composed.prompt, proposalTools }
}
