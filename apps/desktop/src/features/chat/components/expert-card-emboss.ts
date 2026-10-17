import foxMaskUrl from './expert-card-fox-mask.png'

const layers = new WeakMap<HTMLCanvasElement, { key: string; base: HTMLCanvasElement }>()
let foxMask: HTMLImageElement | undefined
const imageListeners = new Set<() => void>()
function loadFoxMask() {
  if (!foxMask) {
    foxMask = new Image()
    foxMask.onload = () => imageListeners.forEach(refresh => refresh())
    foxMask.src = foxMaskUrl
  }
  return foxMask
}

// The reference uses a 260 x 360 frosted back; both renderers share this texture.
export function paintExpertCardBack(canvas: HTMLCanvasElement, host: HTMLElement) {
  const ctx = canvas.getContext('2d')
  if (!ctx) return
  const primary = getComputedStyle(host).getPropertyValue('--primary').trim() || '#6b7280'
  const image = loadFoxMask()
  const key = `${primary}|${canvas.width}|${canvas.height}|${image.complete && image.naturalWidth > 0}`
  let layer = layers.get(canvas)
  if (!layer || layer.key !== key) {
    const base = document.createElement('canvas')
    base.width = canvas.width; base.height = canvas.height
    drawFrostedCard(base, primary, image)
    layer = { key, base }; layers.set(canvas, layer)
  }
  ctx.setTransform(1, 0, 0, 1, 0, 0)
  ctx.clearRect(0, 0, canvas.width, canvas.height)
  ctx.drawImage(layer.base, 0, 0)

}

function drawFrostedCard(canvas: HTMLCanvasElement, primary: string, image: HTMLImageElement) {
  const ctx = canvas.getContext('2d')!
  ctx.fillStyle = '#ffffff'; ctx.fillRect(0, 0, canvas.width, canvas.height)
  ctx.scale(canvas.width / 260, canvas.height / 360)
  const tint = document.createElement('canvas')
  tint.width = 260; tint.height = 360
  const color = tint.getContext('2d')!
  color.fillStyle = primary; color.fillRect(0, 0, 260, 360)
  const mask = document.createElement('canvas')
  mask.width = 260; mask.height = 360
  const maskContext = mask.getContext('2d')!
  const pixels = maskContext.createImageData(260, 360)
  // Offset soft pools preserve white space between them without circular edges.
  for (let y = 0; y < 360; y++) {
    for (let x = 0; x < 260; x++) {
      const upperLeft = Math.exp(-1.4 * (((x - 42) / 145) ** 2 + ((y - 140) / 145) ** 2))
      const lowerRight = Math.exp(-1.4 * (((x - 224) / 178) ** 2 + ((y - 306) / 182) ** 2))
      pixels.data[(y * 260 + x) * 4 + 3] = Math.round(255 * (.02 + .24 * upperLeft + .18 * lowerRight))
    }
  }
  maskContext.putImageData(pixels, 0, 0)
  color.globalCompositeOperation = 'destination-in'
  color.drawImage(mask, 0, 0)
  ctx.drawImage(tint, 0, 0)
  const panel = new Path2D()
  panel.roundRect(22, 44, 216, 294, 12)
  ctx.fillStyle = 'rgba(255,255,255,.12)'; ctx.fill(panel)
  ctx.strokeStyle = 'rgba(255,255,255,.28)'; ctx.lineWidth = 1; ctx.stroke(panel)
  // Seeded grain keeps the frosted texture stable across repaints.
  ctx.save(); ctx.clip(panel)
  let seed = 17
  for (let i = 0; i < 6500; i++) {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0
    const x = seed % 260
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0
    const y = seed % 360
    ctx.fillStyle = i % 2 ? 'rgba(255,255,255,.12)' : 'rgba(0,0,0,.025)'
    ctx.fillRect(x, y, .5, .5)
  }
  ctx.restore()
  if (image.complete && image.naturalWidth) {
    const mark = document.createElement('canvas')
    mark.width = 440; mark.height = 336
    const ink = mark.getContext('2d')!
    const scale = Math.min(mark.width / image.naturalWidth, mark.height / image.naturalHeight)
    const w = image.naturalWidth * scale, h = image.naturalHeight * scale
    ink.drawImage(image, (mark.width - w) / 2, (mark.height - h) / 2, w, h)
    ink.globalCompositeOperation = 'source-in'
    ink.fillStyle = '#ffffff'; ink.fillRect(0, 0, mark.width, mark.height)
    ctx.save(); ctx.globalAlpha = .95
    ctx.shadowColor = primary; ctx.shadowBlur = 10; ctx.shadowOffsetY = 4
    ctx.drawImage(mark, 75, 145.2, 110, 84)
    ctx.restore()
  }
}

export function createExpertCardBackPainter(canvas: HTMLCanvasElement, host: HTMLElement, onPaint: () => void = () => {}) {
  let alive = true
  const refresh = () => { if (alive) { paintExpertCardBack(canvas, host); onPaint() } }
  const theme = new MutationObserver(refresh)
  theme.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'data-color-theme'] })
  imageListeners.add(refresh)
  refresh()
  return {
    setActive(value: boolean) { if (value) refresh() },
    refresh,
    dispose() {
      alive = false
      theme.disconnect()
      imageListeners.delete(refresh)
      layers.delete(canvas)
    },
  }
}
