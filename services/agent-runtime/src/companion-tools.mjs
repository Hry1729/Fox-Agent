const noteTools = new Set(['memory_write', 'note_organize', 'note_search', 'note_read'])
const readerTools = new Set(['note_search', 'note_read'])
const ownerTaskTools = new Set(['companion_task_create', 'companion_task_update', 'companion_task_list', 'companion_task_history', 'companion_task_pause', 'companion_task_resume', 'companion_task_run_now', 'companion_task_delete'])

export function selectCompanionTools(catalog, diagnostics, context) {
  const owner = context?.role === 'owner'
  const reader = context?.role === 'reader'
  const canUpdateProfile = context?.canUpdateProfile === true
  const canWriteNotes = context?.canWriteNotes !== false
  const allowed = new Set(context?.allowedTools ?? [])
  const general = new Set(diagnostics.tools.map(tool => tool.name))
  return catalog.filter(({ name }) => {
    if (name === 'companion_profile_update' || ownerTaskTools.has(name)) return owner && canUpdateProfile && allowed.has(name)
    if (noteTools.has(name)) return owner ? (readerTools.has(name) || canWriteNotes) && allowed.has(name) : reader && readerTools.has(name)
    return general.has(name) && (!owner || allowed.has(name))
  })
}
