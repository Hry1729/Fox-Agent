// The team tool gate follows OpenMausBot's CHIEF_ONLY_TOOL_NAMES in server/drivers/agents-catalog.ts at 1a4e4da9. See third_party/openmausbot/FOX-PORT.md.
const noteTools = new Set(['memory_write', 'note_organize', 'note_search', 'note_read'])
const readerTools = new Set(['note_search', 'note_read'])
const ownerTaskTools = new Set(['companion_task_create', 'companion_task_update', 'companion_task_list', 'companion_task_history', 'companion_task_pause', 'companion_task_resume', 'companion_task_run_now', 'companion_task_delete'])
export const COMPANION_TEAM_TOOLS = new Set([
  'companion_list', 'companion_create', 'companion_propose_profile',
  'companion_propose_model', 'companion_propose_deletion', 'companion_propose_archive',
])
const coordinationTools = new Set(['companion_coordinate', 'companion_assignment_status'])
const chiefNotificationTools = new Set([
  'companion_list_unread_notifications', 'companion_mark_notifications_read', 'companion_retry_failed_task',
])

export function canManageCompanionTeam(context) {
  return context?.role === 'owner' && context.isDefault === true && context.canUpdateProfile === true
}

export function selectCompanionTools(catalog, diagnostics, context, coordination) {
  const owner = context?.role === 'owner'
  const reader = context?.role === 'reader'
  const canUpdateProfile = context?.canUpdateProfile === true
  const canWriteNotes = context?.canWriteNotes !== false
  const allowed = new Set(context?.allowedTools ?? [])
  const coordinationAllowed = new Set(coordination?.allowedTools)
  const canManage = canManageCompanionTeam(context)
  const consultation = coordination?.incoming?.consult
  const general = new Set(diagnostics.tools.map(tool => tool.name))
  return catalog.filter(({ name }) => {
    if (consultation && !coordinationAllowed.has(name)) return false
    if (coordinationTools.has(name)) {
      return !consultation && coordinationAllowed.has(name) && (name !== 'companion_coordinate' || owner)
    }
    if (chiefNotificationTools.has(name)) return canManage && !consultation && coordinationAllowed.has(name)
    if (name === 'companion_list') return coordinationAllowed.has(name) || (canManage && allowed.has(name))
    if (COMPANION_TEAM_TOOLS.has(name)) return canManage && !consultation && allowed.has(name)
    if (name === 'companion_profile_update' || ownerTaskTools.has(name)) return owner && canUpdateProfile && allowed.has(name)
    if (noteTools.has(name)) return owner ? (readerTools.has(name) || canWriteNotes) && allowed.has(name) : reader && readerTools.has(name)
    return general.has(name) && (!owner || allowed.has(name))
  })
}
