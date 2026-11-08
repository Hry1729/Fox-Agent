import { readFileSync, writeFileSync, existsSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const iconDir = join(here, '../../../apps/desktop/node_modules/lucide-react/dist/esm/icons')
const names = `panel-left panel-right sparkles book-copy search bell bell-off message-circle-plus users blocks plus settings sun moon
chevron-down chevron-up chevron-right chevron-left chevrons-up-down arrow-left arrow-down arrow-up x check circle-check circle-alert triangle-alert
loader-circle ellipsis smile reply copy star message-square-dot quote calendar-clock clock history play pause square trash-2 pencil
globe cable folder-lock folder-open file-text file-spreadsheet file-code file-type file presentation download external-link corner-down-left
info shield-check lock palette paperclip archive pin user-round list-filter circle-dot circle refresh-cw rotate-ccw link bot inbox minus arrow-right message-square github`.split(/\s+/).filter(Boolean)

const out = {}
const missing = []
for (const name of names) {
  const file = join(iconDir, `${name}.js`)
  if (!existsSync(file)) { missing.push(name); continue }
  const src = readFileSync(file, 'utf8')
  const match = src.match(/const __iconNode = (\[[\s\S]*?\]);\n/)
  const nodes = Function(`return ${match[1]}`)()
  out[name] = nodes.map(([tag, attrs]) => {
    const { key, ...rest } = attrs
    return `<${tag} ${Object.entries(rest).map(([k, v]) => `${k}="${v}"`).join(' ')}/>`
  }).join('')
}
writeFileSync(join(here, 'icons.js'), `// Generated from lucide-react (ISC license).\nwindow.FOX_ICONS = ${JSON.stringify(out, null, 0)};\n`)
console.log('icons:', Object.keys(out).length, 'missing:', missing.join(',') || 'none')
