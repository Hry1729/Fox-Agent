/**
 * The chat engine preference (migration step 5B). `legacy` is the pre-5B
 * Timeline/Composer implementation and stays the default until the migration's
 * alignment checklist has fully passed; `assistant-ui` renders the new
 * external-store-driven thread instead.
 */
export type FoxChatEngine = 'legacy' | 'assistant-ui'

import './fox-chat-engine.css'

export const FOX_CHAT_ENGINE_KEY = 'fox.ui.chatEngine'

export function readChatEnginePreference(): FoxChatEngine {
  if (typeof window === 'undefined') return 'legacy'
  return window.localStorage.getItem(FOX_CHAT_ENGINE_KEY) === 'assistant-ui' ? 'assistant-ui' : 'legacy'
}

export function writeChatEnginePreference(engine: FoxChatEngine) {
  if (typeof window === 'undefined') return
  window.localStorage.setItem(FOX_CHAT_ENGINE_KEY, engine)
}
