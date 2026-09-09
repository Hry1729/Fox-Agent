// Native engines must honor supported frozen options or reject them explicitly.
export function nativeModelSettings(service) {
  const profile = service.modelProfile ?? {}
  if (!profile || typeof profile !== 'object' || Array.isArray(profile)
      || Object.keys(profile).some(key => !['reasoning', 'thinkingLevel'].includes(key))) {
    throw new Error('Native Kernel adapter does not support these model profile overrides')
  }
  const settings = { ...service, ...profile }
  if (settings.reasoning !== undefined && typeof settings.reasoning !== 'boolean') throw new Error('Invalid native reasoning setting')
  const maxTokens = settings.maxOutputTokens ?? 8192
  const context = settings.contextWindow ?? 128000
  if (!Number.isSafeInteger(maxTokens) || maxTokens < 1 || maxTokens > 131072
      || !Number.isSafeInteger(context) || context < 4096 || context > 4000000) throw new Error('Invalid native token limits')
  return settings
}
