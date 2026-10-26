export interface NetworkTarget { host: string; port: number }
export interface AgentPreview { pid: number; sessionKey: string; port: number; url: string }
export interface AgentNetworkStatus {
  previews: AgentPreview[]
  conversationRules: NetworkTarget[]
  projectRules: NetworkTarget[]
  projectId: string | null
}
export interface AgentNetworkClient {
  agentBackendNetworkStatus: (conversationId: string) => Promise<AgentNetworkStatus>
  agentBackendPreviewAction: (conversationId: string, preview: AgentPreview, stop: boolean) => Promise<boolean>
  agentBackendNetworkRule: (conversationId: string, target: NetworkTarget, projectScope: boolean, allowed: boolean) => Promise<boolean>
}
