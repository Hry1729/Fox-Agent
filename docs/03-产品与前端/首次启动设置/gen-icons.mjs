import { readFileSync, writeFileSync, existsSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const iconDir = join(here, '../../../apps/desktop/node_modules/lucide-react/dist/esm/icons')
const names = `panel-left sparkles book-copy search bell house files database orbit list-todo plus settings sun
chevron-down chevron-right chevron-left folder folder-open folder-plus file-text arrow-left arrow-right arrow-up
ellipsis upload history clock sliders-horizontal check circle-check loader-circle circle-alert external-link copy
quote message-circle list-filter arrow-up-down x trash-2 pencil hard-drive layers wand-sparkles zap brain download
info shield-check layout-grid list refresh-cw rotate-ccw star book-open eye settings-2 cloud-download pause play
square-pen send minus square text-search lightbulb lock cpu package-open inbox file-up calendar bot gauge panel-right
corner-down-left circle-dot pin badge-check mouse-pointer-2 scan-search link file-code file-type presentation file-spreadsheet file moon circle palette chevrons-up-down chevron-up circle-dashed monitor-check package-check`.split(/\s+/).filter(Boolean)

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
