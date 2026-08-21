import { mkdir, readdir, writeFile } from 'node:fs/promises'
import { dirname, extname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const desktopRoot = fileURLToPath(new URL('../', import.meta.url))
const avatarDirectory = resolve(desktopRoot, 'public/avatars/defaults')
const outputFile = resolve(desktopRoot, 'src/generated/profile-avatars.ts')
const supportedExtensions = new Set(['.avif', '.jpeg', '.jpg', '.png', '.svg', '.webp'])

const entries = await readdir(avatarDirectory, { withFileTypes: true }).catch((error) => {
  if (error?.code === 'ENOENT') return []
  throw error
})
const avatarUrls = entries
  .filter((entry) => entry.isFile() && supportedExtensions.has(extname(entry.name).toLowerCase()))
  .map((entry) => `/avatars/defaults/${entry.name}`)
  .sort((left, right) => left.localeCompare(right))
const source = `export const profileAvatarUrls: readonly string[] = ${JSON.stringify(avatarUrls, null, 2)}\n`

await mkdir(dirname(outputFile), { recursive: true })
await writeFile(outputFile, source, 'utf8')
