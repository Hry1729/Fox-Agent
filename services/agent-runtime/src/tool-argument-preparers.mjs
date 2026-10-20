// Argument preparers shared by the legacy Node host tools and the Kernel live
// loop. A provider sometimes serializes an array-valued argument as a JSON
// string; the legacy tool repaired that in `prepareArguments`, while the Kernel
// live loop forwarded the raw string to the Host, which then refused the call.
//
// Strict on purpose: only a string that parses to an array replaces the field.
// Anything else (malformed JSON, an object, an oversized string) is left
// unchanged, so the Host refuses it as a correctable tool error the model sees
// ("tool errors are data") instead of the worker guessing.

export function preparePlanRevisionArguments(args) {
  if (!args || typeof args !== 'object' || Array.isArray(args) || typeof args.tasks !== 'string') {
    return args
  }
  const serializedTasks = args.tasks.trim()
  if (!serializedTasks.startsWith('[') || serializedTasks.length > 400_000) return args
  try {
    const tasks = JSON.parse(serializedTasks)
    return Array.isArray(tasks) ? { ...args, tasks } : args
  } catch {
    return args
  }
}

// Exactly the Host tools whose legacy definition carries this repair. Scope
// follows legacy `preparePlanRevisionArguments`; nothing else is rewritten.
export const KERNEL_TOOL_ARGUMENT_PREPARERS = Object.freeze({
  plan_revision_create: preparePlanRevisionArguments,
})

/**
 * Normalize the arguments of every tool call in an assistant round output
 * before it is committed to the Host, so the durable proposal (and therefore
 * the persisted canonical input, the approval identity and the dispatch
 * payload) is the prepared value. The block is updated in place so the
 * worker's own transcript and the Host's durable history stay identical.
 * Returns the number of rewritten calls.
 */
export function prepareKernelToolCallArguments(message, preparers = KERNEL_TOOL_ARGUMENT_PREPARERS) {
  let rewritten = 0
  for (const block of Array.isArray(message?.content) ? message.content : []) {
    if (block?.type !== 'toolCall' || typeof block.name !== 'string') continue
    const prepare = Object.prototype.hasOwnProperty.call(preparers, block.name) ? preparers[block.name] : null
    if (typeof prepare !== 'function') continue
    const prepared = prepare(block.arguments)
    // A frozen block (never produced by Pi) is left exactly as it is.
    if (prepared !== block.arguments && !Object.isFrozen(block)) {
      block.arguments = prepared
      rewritten += 1
    }
  }
  return rewritten
}
