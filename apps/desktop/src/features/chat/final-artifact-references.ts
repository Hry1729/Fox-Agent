import type { ArtifactRecord } from '../conversations/model/types'

export function displayArtifactPath(path: string) {
  return path.replace(/^[\\/]{2}\?[\\/]UNC[\\/]/iu, '\\\\').replace(/^[\\/]{2}\?[\\/]/u, '')
}

function normalizedArtifactPath(path: string) {
  return displayArtifactPath(path).replaceAll('\\', '/').replace(/^\.\//u, '')
}

export function artifactRelativePath(artifact: ArtifactRecord, projectRoot: string | null | undefined) {
  if (!projectRoot || artifact.artifactOrigin !== 'project') return undefined
  const absolute = normalizedArtifactPath(artifact.storagePath)
  const root = normalizedArtifactPath(projectRoot).replace(/\/$/u, '')
  const windows = /^(?:[a-z]:\/|\/\/)/iu.test(absolute)
  const prefix = `${root}/`
  return (windows ? absolute.toLowerCase().startsWith(prefix.toLowerCase()) : absolute.startsWith(prefix))
    ? absolute.slice(prefix.length)
    : undefined
}

export function resolveFinalArtifactReference(href: string, artifacts: ArtifactRecord[]) {
  if (!href.startsWith('#fox-artifact=')) return undefined
  try {
    const id = decodeURIComponent(href.slice('#fox-artifact='.length))
    const exact = artifacts.find(artifact => artifact.id === id)
    if (exact) return exact
    const path = normalizedArtifactPath(id)
    if (!path || path.split('/').some(part => part === '..')) return undefined
    const matches = artifacts.filter(artifact => {
      if (!path.includes('/')) return artifact.displayName === path
      const storagePath = normalizedArtifactPath(artifact.storagePath)
      return storagePath === path || (!/^(?:\/|[a-z]:)/iu.test(path) && storagePath.endsWith(`/${path}`))
    })
    if (matches.length === 1) return matches[0]
  } catch {
    return undefined
  }
}

/** Selection affects presentation only; Host classification and delivery facts stay intact. */
export function finalArtifactIds(text: string, artifacts: ArtifactRecord[]): Set<string> {
  const selected = new Set<string>()
  const prose = text.replace(/^```[^\n]*\n[\s\S]*?^```\s*$/gm, '')
  for (const link of prose.matchAll(/\[[^\]\n]+\]\((#fox-artifact=[^\s)]+)\)/g)) {
    const artifact = resolveFinalArtifactReference(link[1], artifacts)
    if (artifact) selected.add(artifact.id)
  }
  // Older replies used a delivery heading with bold/code filenames rather than IDs.
  let deliveryLevel = 0
  for (const line of prose.split('\n')) {
    const heading = line.match(/^(#{1,6})\s+(.+?)\s*#*\s*$/)
    if (heading) {
      if (deliveryLevel && heading[1].length <= deliveryLevel) deliveryLevel = 0
      if (/^(?:[一二三四五六七八九十\d]+[、.．]\s*)?(?:交付物|交付文件|本次交付|最终交付物)\s*[:：]?$/.test(heading[2])) deliveryLevel = heading[1].length
      continue
    }
    if (!deliveryLevel || /(?:未生成|未完成|不存在|失败|缺失|无法|待生成)/.test(line)) continue
    for (const match of line.matchAll(/`([^`\n]+)`|\*\*([^*\n]+)\*\*/g)) {
      const name = match[1] ?? match[2]
      const candidates = artifacts.filter(artifact => artifact.displayName === name)
      if (candidates.length === 1) selected.add(candidates[0].id)
    }
  }
  return selected
}
