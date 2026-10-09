// Host environment evidence is independent of the engine's capability schema.
// An absent legacy payload means unknown, never available. This prompt metadata
// grants no authorization: Host checks the production proof on every dispatch.
export function hostRuntimeCapabilities(value, { capabilityManifestHash, executionProfileId } = {}) {
  if (!value || value.schemaVersion !== 1 || !value.tools || typeof value.tools !== 'object'
      || typeof value.capabilityManifestHash !== 'string' || typeof value.executionProfileId !== 'string'
      || (capabilityManifestHash && value.capabilityManifestHash !== capabilityManifestHash)
      || (executionProfileId && value.executionProfileId !== executionProfileId)) return null
  const tools = {}
  for (const [name, item] of Object.entries(value.tools).slice(0, 64)) {
    if (!/^[a-zA-Z0-9_]{1,80}$/.test(name) || !['available', 'unavailable', 'unknown'].includes(item?.availability)) continue
    tools[name] = { availability: item.availability,
      reasonCode: typeof item.reasonCode === 'string' ? item.reasonCode.slice(0, 100) : null,
      reason: typeof item.reason === 'string' ? item.reason.slice(0, 600) : null,
      actions: item.actions && typeof item.actions === 'object'
        ? Object.fromEntries(Object.entries(item.actions).filter(([action, state]) => /^[a-z_]{1,30}$/.test(action) && ['available', 'unavailable', 'unknown'].includes(state))) : undefined }
    if (name === 'attachment_compute') {
      tools[name].projectPaths = item.projectPaths === true
      if (['available','unavailable','unknown'].includes(item.reportPdf?.availability)) {
        const pdf=item.reportPdf
        tools[name].reportPdf=Object.fromEntries(['availability','reasonCode','reason','renderer','font','fontSha256','fontEmbedding','maxPages','maxSpecBytes','chartTypes']
          .filter(key=>pdf[key]!==undefined).map(key=>[key,typeof pdf[key]==='string'?pdf[key].slice(0,600):pdf[key]]))
      }
    }
  }
  return { schemaVersion: 1, capabilityManifestHash: value.capabilityManifestHash, executionProfileId: value.executionProfileId, tools }
}

export function hostRuntimeCapabilitiesPrompt(value, identity) {
  const facts = hostRuntimeCapabilities(value, identity)
  if (!facts) return ''
  return `Host runtime availability for this frozen manifest/profile:\n${JSON.stringify(facts)}\nTool schemas describe contracts and do not prove environment availability. Do not call an unavailable action or repeat it to seek approval; approval cannot supply a missing backend. Use the available in-process tools. Command status/output/cancel are control-plane actions and do not prove command start is available. Report an unavailable required capability clearly. Capabilities can change; Host rechecks before approval and before execution.`
}
