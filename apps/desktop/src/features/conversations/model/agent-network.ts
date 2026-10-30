export interface AgentPreview { pid: number; sessionKey: string; port: number; url: string }
export interface AgentNetworkStatus {
  previews: AgentPreview[]
  projectId: string | null
}
export interface AgentNetworkClient {
  agentBackendNetworkStatus: (conversationId: string) => Promise<AgentNetworkStatus>
  agentBackendPreviewAction: (conversationId: string, preview: AgentPreview, stop: boolean) => Promise<boolean>
}
