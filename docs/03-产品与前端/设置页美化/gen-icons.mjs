import { readFileSync, writeFileSync, existsSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const iconDir = join(here, '../../../apps/desktop/node_modules/lucide-react/dist/esm/icons')
const names = `panel-left menu arrow-left arrow-right minus square x chevron-down chevron-right check ellipsis plus search sun settings
sliders-horizontal message-circle-more boxes globe bot blocks library-big database server puzzle cable shield-check shield wrench
folder-open rotate-ccw zap pencil trash-2 external-link key-round circle-check circle-alert circle-dashed loader-circle info log-in
archive archive-restore eye eye-off hard-drive list-checks layers sparkles user-round folder-lock git-branch lock triangle-alert
refresh-cw file-text circle-x square-terminal link users play`.split(/\s+/).filter(Boolean)

const out = {}
const missing = []
for (const name of names) {
  const file = join(iconDir, `${name}.js`)
  if (!existsSync(file)) { missing.push(name); continue }
  let src = readFileSync(file, 'utf8')
  // Renamed icons are re-exports of the current file name.
  const alias = src.match(/from '\.\/([\w-]+)\.js'/)
  if (!src.includes('__iconNode') && alias) src = readFileSync(join(iconDir, `${alias[1]}.js`), 'utf8')
  const match = src.match(/const __iconNode = (\[[\s\S]*?\]);\r?\n/)
  const nodes = Function(`return ${match[1]}`)()
  out[name] = nodes.map(([tag, attrs]) => {
    const { key, ...rest } = attrs
    return `<${tag} ${Object.entries(rest).map(([k, v]) => `${k}="${v}"`).join(' ')}/>`
  }).join('')
}

writeFileSync(join(here, 'icons.js'), `// Generated from lucide-react (ISC license).\nwindow.FOX_ICONS = ${JSON.stringify(out, null, 0)};\n`)
console.log('icons:', Object.keys(out).length, 'missing:', missing.join(',') || 'none')
