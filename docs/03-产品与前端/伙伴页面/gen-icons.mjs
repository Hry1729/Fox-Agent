import { readFileSync, writeFileSync, existsSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const iconDir = join(here, '../../../apps/desktop/node_modules/lucide-react/dist/esm/icons')
const names = `panel-left panel-right sparkles book-copy paw-print search bell plus settings sun menu minus square x
chevron-down chevron-right chevron-left arrow-left arrow-right arrow-up check check-check ellipsis folder folder-open folder-lock
message-circle-plus users blocks pin pin-off pencil archive archive-restore trash-2 user-round user-plus mail mail-open
grip-vertical loader-circle circle-alert file-text file-spreadsheet calendar-clock list-checks book-open clock external-link
layout-dashboard bot sliders-horizontal briefcase-business layers smile reply star star-off panel-left-close square-pen
arrow-up-down copy eye-off bell-off circle-check square-kanban`.split(/\s+/).filter(Boolean)

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
// Not in lucide: the 伙伴 tab glyph (lucide's paw-print has three toes; this
// one has four). Mirrors apps/desktop/src/components/animate-ui/icons/cat-paw.tsx.
const PAW_PAD = 'M-3.6 2.1C-3.6-.7-1.7-1.7 0-1.7C1.7-1.7 3.6-.7 3.6 2.1C3.6 3.9 2.5 4.6 1.5 4.6C.8 4.6.5 4.2 0 4.2C-.5 4.2-.8 4.6-1.5 4.6C-2.5 4.6-3.6 3.9-3.6 2.1Z'
const PAW_TOES = [[-4.6, -1.9, -30], [-1.75, -4.4, -10], [1.75, -4.4, 10], [4.6, -1.9, 30]]
const PAW = { x: 12.634, y: 13.41, rotate: -10, scale: 1.609, toe: { rx: 1.45, ry: 1.8, spread: 1.03 } }
const r3 = (n) => Math.round(n * 1000) / 1000
out['cat-paw'] = `<g transform="translate(${PAW.x} ${PAW.y}) rotate(${PAW.rotate})"><g class="paw"><path d="${PAW_PAD.replace(/-?\d*\.?\d+/g, (n) => r3(n * PAW.scale))}"/>${PAW_TOES.map(([x, y, r]) => {
  const cx = r3(x * PAW.scale * PAW.toe.spread), cy = r3(y * PAW.scale * PAW.toe.spread)
  return `<ellipse cx="${cx}" cy="${cy}" rx="${r3(PAW.toe.rx * PAW.scale)}" ry="${r3(PAW.toe.ry * PAW.scale)}" transform="rotate(${r} ${cx} ${cy})"/>`
}).join('')}</g></g>`

writeFileSync(join(here, 'icons.js'), `// Generated from lucide-react (ISC license).\nwindow.FOX_ICONS = ${JSON.stringify(out, null, 0)};\n`)
console.log('icons:', Object.keys(out).length, 'missing:', missing.join(',') || 'none')
