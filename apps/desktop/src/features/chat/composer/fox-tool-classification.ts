/**
 * Tool-name classification shared by the composer, the timeline process model
 * and the right panel. Kept in its own dependency-free module so pure data
 * code (e.g. the assistant-ui thread converter) can use it without pulling
 * the composer's component tree into the import graph.
 */

export function isFileChangeTool(toolName: string) {
  return toolName === 'write_file' || toolName === 'edit_file'
}

export function isCommandTool(toolName: string) {
  return /^(run_command|test_run|code_check|format_code|shell|command)$/i.test(toolName)
}

export function isChangeOrCommandTool(toolName: string) {
  return isFileChangeTool(toolName) || isCommandTool(toolName)
}
