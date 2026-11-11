import { readFile, writeFile, mkdir, copyFile } from 'node:fs/promises'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'

const root = fileURLToPath(new URL('../', import.meta.url))
const artwork = await readFile(join(root, 'assets/app-icon-artwork.png'))
const output = join(root, 'src-tauri/icons')
const source = join(output, 'app-icon.svg')
await mkdir(output, { recursive: true })
await writeFile(source, `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="512" height="512" viewBox="0 0 512 512"><rect width="512" height="512" rx="92" fill="#fff"/><image x="32" y="32" width="448" height="448" preserveAspectRatio="xMidYMid meet" xlink:href="data:image/png;base64,${artwork.toString('base64')}"/></svg>\n`)
const generated = spawnSync(process.execPath, [join(root, 'node_modules/@tauri-apps/cli/tauri.js'), 'icon', source, '--output', output], { cwd: root, stdio: 'inherit', windowsHide: true })
if (generated.status !== 0) process.exit(generated.status ?? 1)
await copyFile(join(output, 'icon.png'), join(root, 'public/app-icon.png'))
