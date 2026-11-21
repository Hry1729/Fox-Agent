import { readFileSync, writeFileSync, existsSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const iconDir = join(here, '../../../apps/desktop/node_modules/lucide-react/dist/esm/icons')
const names = `panel-left panel-right sparkles book-copy search bell plus settings sun moon menu minus square x
chevron-down chevron-up chevron-right chevrons-up-down arrow-left arrow-right arrow-up check circle-check circle circle-dashed
loader-circle ellipsis file-text file-type file-code file-spreadsheet file presentation folder folder-open folders globe-2 book-open bot
square-terminal list-checks activity layout-dashboard maximize-2 minimize-2 cpu message-circle-plus users blocks shield-check
circle-stop external-link paperclip history file-pen palette brain copy refresh-cw user-round clock layers circle-help
triangle-alert rotate-ccw trash-2 target chevron-left lock eye quote circle-x timer list-tree arrow-up-right link-2 toggle-right hard-drive git-compare-arrows
pause play info pencil file-plus grip-vertical arrow-up-down`.split(/\s+/).filter(Boolean)

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
