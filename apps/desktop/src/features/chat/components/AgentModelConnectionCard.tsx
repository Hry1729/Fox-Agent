// Types of the Host's `agent_backend_model_connection` answer. The card that
// used to sit above the composer is gone (UI 1103); 设置 → Agent reads the
// status to say whether an external Agent is configured.
import type { AgentBackendTarget } from '@/features/conversations/model/agent-backend-onboarding'
export interface AgentModelConnection {
  status: 'existing_login' | 'ready' | 'missing_provider' | 'missing_address' | 'choose_address' | 'missing_key' | 'verification_failed'
  providerId: string | null
  providerName: string | null
  apiType: string
  candidates: Array<{ id: string; baseUrl: string; modelId: string }>
  message: string | null
}
export interface AgentModelConnectionRequest {
  target: AgentBackendTarget
  providerId?: string
  profileId?: string
  baseUrl?: string
}
