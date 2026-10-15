// Coordinates are shared by the Three texture and the non-WebGL card face.
const shapes = [
  'M279 158 C297 149 316 137 334 129 C333 150 326 169 304 178 C296 177 285 169 279 158 Z',
  'M290 182 C312 172 337 174 352 187 C336 204 310 208 290 193 Z',
  'M44 132 C31 106 24 60 28 24 C29 12 34 9 43 14 C67 27 91 48 112 76 C143 63 189 63 220 76 C241 48 265 27 289 14 C298 9 303 12 304 24 C308 60 301 106 288 132 C295 146 305 157 315 165 Q305 172 296 172 Q304 179 314 182 C300 192 287 194 276 195 C249 213 212 218 166 216 C120 218 83 213 56 195 C45 194 32 192 18 182 Q28 179 36 172 Q27 172 17 165 C27 157 37 146 44 132 Z',
  'M51 108 C44 88 41 60 44 40 C62 49 79 65 92 82 C76 87 62 96 51 108 Z',
  'M240 82 C253 65 270 49 288 40 C291 60 288 88 281 108 C270 96 256 87 240 82 Z',
]

const layers = new WeakMap<HTMLCanvasElement, { key: string; base: HTMLCanvasElement; mask: Path2D }>()

export function paintExpertCardBack(canvas: HTMLCanvasElement, host: HTMLElement, phase = .5) {
  const ctx = canvas.getContext('2d')
  if (!ctx) return
  const styles = getComputedStyle(host)
  const background = styles.getPropertyValue('--shell').trim() || '#f4f4f4'
  const ink = styles.getPropertyValue('--muted-foreground').trim() || '#68707b'
  const key = `${background}|${ink}|${canvas.width}|${canvas.height}|${document.fonts.status}`
  let layer = layers.get(canvas)
  if (!layer || layer.key !== key) {
    const base = document.createElement('canvas')
    base.width = canvas.width; base.height = canvas.height
    const mask = drawSilverCard(base, background, ink)
    layer = { key, base, mask }; layers.set(canvas, layer)
  }
  ctx.setTransform(1, 0, 0, 1, 0, 0)
  ctx.drawImage(layer.base, 0, 0)
  ctx.save()
  ctx.scale(canvas.width / 512, canvas.height / 686)
  ctx.translate(73, 219)
  ctx.clip(layer.mask)
  const x = -180 + phase * 720
  const sweep = ctx.createLinearGradient(x, 0, x + 150, 90)
  sweep.addColorStop(0, 'rgba(255,255,255,0)')
  sweep.addColorStop(.4, 'rgba(255,255,255,.12)')
  sweep.addColorStop(.55, 'rgba(255,255,255,.7)')
  sweep.addColorStop(1, 'rgba(255,255,255,0)')
  ctx.fillStyle = sweep; ctx.fillRect(0, 0, 370, 250)
  ctx.restore()
}

function drawSilverCard(canvas: HTMLCanvasElement, background: string, ink: string) {
  const ctx = canvas.getContext('2d')!
  ctx.fillStyle = background; ctx.fillRect(0, 0, canvas.width, canvas.height)
  ctx.scale(canvas.width / 512, canvas.height / 686)
  const mask = new Path2D()
  ctx.save()
  ctx.translate(73, 219)
  const emboss = (path: Path2D, inset = false) => {
    if (!inset) mask.addPath(path)
    ctx.save()
    ctx.filter = 'blur(1px)'
    ctx.translate(1.8, 1.8)
    ctx.fillStyle = inset ? '#ffffff' : '#b7bdc7'; ctx.fill(path)
    ctx.translate(-3.6, -3.6)
    ctx.fillStyle = inset ? '#b7bdc7' : '#ffffff'; ctx.fill(path)
    ctx.translate(1.8, 1.8)
    ctx.filter = 'none'
    const face = ctx.createLinearGradient(0, 30, 180, 240)
    face.addColorStop(0, '#ffffff'); face.addColorStop(.46, '#f4f6f8')
    face.addColorStop(.64, '#dce1e7'); face.addColorStop(.8, '#f8fafc'); face.addColorStop(1, '#e7ebf0')
    ctx.fillStyle = inset ? '#dbe0e6' : face; ctx.fill(path)
    ctx.restore()
  }
  shapes.forEach(shape => emboss(new Path2D(shape)))
  const oval = (x: number, y: number, rx: number, ry: number, inset = false) => {
    const path = new Path2D()
    path.ellipse(x, y, rx, ry, 0, 0, Math.PI * 2)
    emboss(path, inset)
  }
  oval(113, 164, 10, 12, true); oval(220, 164, 10, 12, true)
  oval(166, 193, 18, 11)
  oval(73, 202, 29, 30); oval(259, 202, 29, 30)
  ctx.restore()
  ctx.fillStyle = ink
  ctx.font = '500 40px "HarmonyOS Sans SC", sans-serif'
  ctx.textAlign = 'center'; ctx.textBaseline = 'alphabetic'
  ctx.letterSpacing = '1px'
  ctx.fillText('Fox Agent', 256, 506)
  ctx.letterSpacing = '0px'
  return mask
}

export function createExpertCardBackPainter(canvas: HTMLCanvasElement, host: HTMLElement, onPaint: () => void = () => {}) {
  const motion = matchMedia('(prefers-reduced-motion: reduce)')
  let alive = true, active = false, visible = true, frame = 0, last = 0
  const refresh = () => { if (alive) { paintExpertCardBack(canvas, host); onPaint() } }
  const canRun = () => alive && active && visible && !motion.matches && document.visibilityState !== 'hidden'
  const tick = (now: number) => {
    frame = 0
    if (!canRun()) return
    if (now - last >= 1000 / 24 && getComputedStyle(host).visibility !== 'hidden') {
      last = now
      paintExpertCardBack(canvas, host, (now % 6500) / 6500); onPaint()
    }
    if (canRun()) frame = requestAnimationFrame(tick)
  }
  const sync = () => {
    cancelAnimationFrame(frame); frame = 0
    refresh()
    if (canRun()) frame = requestAnimationFrame(tick)
  }
  const theme = new MutationObserver(refresh)
  theme.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'data-color-theme'] })
  const viewport = new IntersectionObserver(entries => { visible = entries[0]?.isIntersecting ?? false; sync() })
  viewport.observe(host)
  motion.addEventListener('change', sync)
  document.addEventListener('visibilitychange', sync)
  void document.fonts.ready.then(refresh)
  refresh()
  return {
    setActive(value: boolean) { active = value; sync() },
    refresh,
    dispose() {
      alive = false; cancelAnimationFrame(frame); theme.disconnect(); viewport.disconnect()
      motion.removeEventListener('change', sync); document.removeEventListener('visibilitychange', sync)
      layers.delete(canvas)
    },
  }
}
