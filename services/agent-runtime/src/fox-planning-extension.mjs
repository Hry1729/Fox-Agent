import { compactWorkSnapshot } from './prompt-composer.mjs'

export function createFoxPlanningExtension({ workTools = [], context = {} } = {}) {
  // The planning extension may consume the same structured object for local
  // decisions, but composeFoxPrompt owns its sole model-visible rendering.
  const workSnapshot = context.workSnapshot ?? null
  return (pi) => {
    void workSnapshot
    for (const tool of workTools) pi.registerTool(tool)
  }
}

export { compactWorkSnapshot }
