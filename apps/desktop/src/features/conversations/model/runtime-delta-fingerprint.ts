/** The Host emits this UTF-8 FNV-1a fingerprint alongside each answer length. */
export function answerDeltaFingerprint(text: string): string {
  let hash = 0xcbf29ce484222325n
  for (const byte of new TextEncoder().encode(text)) {
    hash = BigInt.asUintN(64, (hash ^ BigInt(byte)) * 0x100000001b3n)
  }
  return hash.toString(16).padStart(16, '0')
}
