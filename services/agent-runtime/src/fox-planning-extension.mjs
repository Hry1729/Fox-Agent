import { compactWorkSnapshot } from './prompt-composer.mjs'

export function createFoxPlanningExtension({ workTools = [], context = {}, promptSections = null } = {}) {
  // The planning extension may consume the same structured object for local
  // decisions, but composeFoxPrompt owns its sole model-visible rendering.
  const workSnapshot = context.workSnapshot ?? null
  return (pi) => {
    void workSnapshot
    for (const tool of workTools) pi.registerTool(tool)
    if (promptSections) {
      // The stable contract and the per-turn context are separate SDK prompt
      // sections. A changed turn tail then patches only `fox_context` instead
      // of replaying the full preamble on every turn.
      pi.on('before_agent_start', (event) => {
        event.systemPromptOptions.customPrompt = promptSections.stable
        event.systemPromptOptions.sections = {
          ...event.systemPromptOptions.sections,
          ...(promptSections.dynamic ? { fox_context: promptSections.dynamic } : {}),
        }
      })
    }
  }
}

export { compactWorkSnapshot }
