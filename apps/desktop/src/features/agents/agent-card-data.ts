import type { AgentRecord } from '@/features/conversations/model/types'
import type { AgentCardData } from '@/features/workspace/entity-card'
import { resolveExpertKnowledgeDeclaration } from './expert-knowledge'

export function asAgentCard(agent: AgentRecord): AgentCardData {
  const manifest = agent.packageManifest
  let knowledgeCount = agent.resources.knowledges.length
  if (agent.runtimeType === 'pi') {
    try {
      const declaration = resolveExpertKnowledgeDeclaration(manifest)
      knowledgeCount = declaration.format === 'references'
        ? declaration.references.length
        : declaration.format === 'legacy_remote_ids'
          ? declaration.ids.length
          : 0
    } catch {
      knowledgeCount = 0
    }
  }
  return {
    ...agent,
    image: agent.icon || (agent.runtimeType === 'pi' ? '/mascot/fox_magic.png' : '/mascot/fox_search.png'),
    tools: agent.runtimeType === 'pi' && Array.isArray(manifest.allowedTools) ? manifest.allowedTools.length : agent.resources.tools.length,
    knowledge: knowledgeCount,
    mcps: agent.runtimeType === 'pi' && Array.isArray(manifest.mcpServers) ? manifest.mcpServers.length : agent.resources.mcps.length,
    skills: agent.runtimeType === 'pi' && Array.isArray(manifest.skills) ? manifest.skills.length : agent.resources.skills.length,
    recent: agent.runtimeType === 'pi' ? '本机' : '知识库',
    active: agent.isDefault,
  }
}
