const noteTools = new Set(['memory_write', 'note_organize', 'note_search', 'note_read'])
const readerTools = new Set(['note_search', 'note_read'])

export function selectCompanionTools(catalog, diagnostics, context) {
  const owner = context?.role === 'owner'
  const reader = context?.role === 'reader'
  const allowed = new Set(context?.allowedTools ?? [])
  const general = new Set(diagnostics.tools.map(tool => tool.name))
  return catalog.filter(({ name }) => {
    if (noteTools.has(name)) return owner ? allowed.has(name) : reader && readerTools.has(name)
    return general.has(name) && (!owner || allowed.has(name))
  })
}
