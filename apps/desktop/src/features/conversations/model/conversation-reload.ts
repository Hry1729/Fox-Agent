export async function loadConversationWithRetry<T>(
  load: (conversationId: string) => Promise<T>,
  conversationId: string,
  { attempts = 3, delayMs = 120 }: { attempts?: number; delayMs?: number } = {},
) {
  const totalAttempts = Math.max(1, Math.floor(attempts))
  let lastError: unknown
  for (let attempt = 1; attempt <= totalAttempts; attempt += 1) {
    try {
      return await load(conversationId)
    } catch (error) {
      lastError = error
      if (attempt < totalAttempts && delayMs > 0) {
        await new Promise((resolve) => window.setTimeout(resolve, delayMs * attempt))
      }
    }
  }
  throw lastError instanceof Error
    ? lastError
    : new Error(String(lastError ?? 'Failed to reload the conversation.'))
}
