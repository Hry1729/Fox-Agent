// Prompt preflight for the non-compressible part of a Pi request.
//
// The provider tokenizer is not available offline, so this is deliberately a
// conservative lower bound: CJK code points count as one token each and the
// remaining characters count as one token per four characters. That matches
// the 29,823-character / 17,886-token dbd61248 Chinese sample closely enough to
// reject an impossible 16K request before the SDK sends it.

function isCjkCodePoint(codePoint) {
  return (codePoint >= 0x2e80 && codePoint <= 0x2fff)
    || (codePoint >= 0x3000 && codePoint <= 0x303f)
    || (codePoint >= 0x3040 && codePoint <= 0x30ff)
    || (codePoint >= 0x3100 && codePoint <= 0x312f)
    || (codePoint >= 0x31a0 && codePoint <= 0x31bf)
    || (codePoint >= 0x3400 && codePoint <= 0x4dbf)
    || (codePoint >= 0x4e00 && codePoint <= 0x9fff)
    || (codePoint >= 0xf900 && codePoint <= 0xfaff)
    || (codePoint >= 0xfe30 && codePoint <= 0xfe4f)
    || (codePoint >= 0xff00 && codePoint <= 0xffef)
    || (codePoint >= 0x20000 && codePoint <= 0x2fa1f)
}

export function estimatePromptTokens(text) {
  const value = String(text ?? '')
  if (!value) return 0
  let cjk = 0
  for (const character of value) {
    if (isCjkCodePoint(character.codePointAt(0))) cjk += 1
  }
  const remaining = Math.max(0, [...value].length - cjk)
  return cjk + Math.ceil(remaining / 4)
}

export function promptTokenPreflight({
  systemPrompt = '',
  toolDefinitions = '',
  userText = '',
  contextWindow,
  reserveTokens,
}) {
  const estimatedTokens = estimatePromptTokens(
    [systemPrompt, toolDefinitions, userText].filter(Boolean).join('\n'),
  )
  const availableInputTokens = Math.max(0, Number(contextWindow) - Number(reserveTokens))
  return {
    estimatedTokens,
    availableInputTokens,
    contextWindow: Number(contextWindow),
    reserveTokens: Number(reserveTokens),
    exceeds: estimatedTokens > availableInputTokens,
  }
}
