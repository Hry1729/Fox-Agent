/* React Bits Lanyard — MIT + Commons Clause License Condition v1.0
 * Copyright (c) 2026 David Haz
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, and distribute the Software as part of an
 * application, website, or product, subject to the following conditions:
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 * You may use this Software, including for any commercial purpose, so long as you
 * do not sell, sublicense, or redistribute the components themselves—whether
 * alone, in a bundle, or as a ported version.
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 * https://github.com/DavidHDev/react-bits/blob/main/LICENSE.md
 */
// https://reactbits.dev/components/lanyard
// Source: https://github.com/DavidHDev/react-bits/blob/3329f3bde763a37a2a89b24598e9f50fa0d4de3d/src/ts-default/Components/Lanyard/Lanyard.tsx
// Retained: rounded extruded card, slot/bevel and UVs, ring/clamp geometry,
// woven strap normal map, PMREM studio panels and glossy/silver/black defaults.
// Fox differences: compact portrait, one fixed strap, named expert on front,
// patterned Fox back and lazy loading. The React shell handles pull-to-toggle.
// No physics, breeze, automatic rotation or idle render loop.
import * as THREE from 'three'
import { createExpertCardBackPainter, paintExpertCardBack } from './expert-card-emboss'

const WIDTH = 1.68, HEIGHT = 2.25, THICKNESS = 0.018, BEVEL = 0.007
const RING_RADIUS = 0.095, RING_TUBE = 0.0135, EYELET_RADIUS = 0.036, CLAMP_BODY = 0.15
const STRAP_WIDTH = 0.36 * 0.65
interface PathLike { moveTo(x: number, y: number): unknown; lineTo(x: number, y: number): unknown; bezierCurveTo(a: number, b: number, c: number, d: number, e: number, f: number): unknown }
const traceRoundedRect = (path: PathLike, x: number, y: number, width: number, height: number, radius: number) => {
  const r = Math.max(0, Math.min(radius, width / 2, height / 2)), k = r * 0.4477
  path.moveTo(x + r, y); path.lineTo(x + width - r, y)
  path.bezierCurveTo(x + width - k, y, x + width, y + k, x + width, y + r)
  path.lineTo(x + width, y + height - r)
  path.bezierCurveTo(x + width, y + height - k, x + width - k, y + height, x + width - r, y + height)
  path.lineTo(x + r, y + height)
  path.bezierCurveTo(x + k, y + height, x, y + height - k, x, y + height - r)
  path.lineTo(x, y + r); path.bezierCurveTo(x, y + k, x + k, y, x + r, y)
}
const buildCardGeometry = () => {
  const radius = 0.03 + (Math.min(WIDTH, HEIGHT) * 0.2 - 0.03) * 0.3
  const outline = new THREE.Shape()
  traceRoundedRect(outline, -WIDTH / 2 + BEVEL, -HEIGHT / 2 + BEVEL, WIDTH - BEVEL * 2, HEIGHT - BEVEL * 2, Math.max(radius - BEVEL, 0.005))
  const hole = new THREE.Path()
  traceRoundedRect(hole, -0.34 / 2 - BEVEL, HEIGHT / 2 - 0.13 - 0.07 / 2 - BEVEL, 0.34 + BEVEL * 2, 0.07 + BEVEL * 2, 0.07 / 2 + BEVEL)
  outline.holes.push(hole)
  const body = new THREE.ExtrudeGeometry(outline, { depth: THICKNESS, bevelEnabled: true, bevelThickness: BEVEL, bevelSize: BEVEL, bevelSegments: 5, curveSegments: 24 })
  body.translate(0, 0, -THICKNESS / 2)
  const face = new THREE.ShapeGeometry(outline, 24)
  const position = face.getAttribute('position'), uv = face.getAttribute('uv')
  for (let i = 0; i < position.count; i++) uv.setXY(i, (position.getX(i) + WIDTH / 2) / WIDTH, (position.getY(i) + HEIGHT / 2) / HEIGHT)
  uv.needsUpdate = true
  return { body, face }
}
const buildClampGeometry = () => {
  const shape = new THREE.Shape(), w = STRAP_WIDTH * 1.16
  traceRoundedRect(shape, -w / 2 + 0.014, -CLAMP_BODY / 2 + 0.014, w - 0.028, CLAMP_BODY - 0.028, 0.024)
  const geometry = new THREE.ExtrudeGeometry(shape, { depth: 0.02, bevelEnabled: true, bevelThickness: 0.014, bevelSize: 0.014, bevelSegments: 6, curveSegments: 10 })
  geometry.translate(0, 0, -0.01)
  return geometry
}
const buildEnvironment = (renderer: THREE.WebGLRenderer) => {
  const scene = new THREE.Scene()
  scene.background = new THREE.Color(0.035, 0.035, 0.04)
  const geometry = new THREE.PlaneGeometry(1, 1), materials: THREE.Material[] = []
  const panel = (intensity: number, position: [number, number, number], scale: [number, number], rotation = 0) => {
    const material = new THREE.MeshBasicMaterial({ color: new THREE.Color(intensity, intensity, intensity), side: THREE.DoubleSide })
    materials.push(material)
    const mesh = new THREE.Mesh(geometry, material)
    mesh.position.set(...position); mesh.scale.set(scale[0], scale[1], 1); mesh.lookAt(0, 0, 0); mesh.rotateZ(rotation); scene.add(mesh)
  }
  panel(2.6, [0, 4.5, 8], [9, 4]); panel(6, [-5.5, 0.5, 7], [0.9, 14]); panel(4, [5, -0.5, 8], [0.6, 14])
  panel(3.5, [0, -1, 9], [24, 0.35], Math.PI / 3); panel(2.5, [1, 2.5, 9], [24, 0.2], Math.PI / 3)
  panel(1.4, [0, 6, -6], [12, 4]); panel(0.5, [0, -7, 1], [16, 6]); panel(0.9, [8, 0, -2], [4, 10])
  const pmrem = new THREE.PMREMGenerator(renderer)
  try { return pmrem.fromScene(scene, 0.035) }
  finally { pmrem.dispose(); geometry.dispose(); materials.forEach(material => material.dispose()); scene.clear() }
}
const seeded = (start: number) => {
  let state = start >>> 0
  return () => { state = (state + 0x6d2b79f5) >>> 0; let t = state; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296 }
}
const buildWeaveTexture = () => {
  const size = 128, threads = 8, random = seeded(19)
  const fiber = Float32Array.from({ length: size * size }, () => random())
  const profile = (f: number) => Math.sqrt(Math.max(0, 1 - 4 * f * f))
  const height = (x: number, y: number) => {
    const px = ((x % size) + size) % size, py = ((y % size) + size) % size
    const u = px / size * threads, v = py / size * threads, cu = Math.floor(u), cv = Math.floor(v), fu = u - cu - 0.5, fv = v - cv - 0.5, lift = (cu + cv) % 2 === 0
    const warp = profile(fu) * (0.55 + 0.45 * Math.cos(fv * Math.PI * (lift ? 1 : 0.6))), weft = profile(fv) * (0.55 + 0.45 * Math.cos(fu * Math.PI * (lift ? 0.6 : 1)))
    return (lift ? Math.max(warp, weft * 0.7) : Math.max(weft, warp * 0.7)) + fiber[py * size + px] * 0.08
  }
  const data = new Uint8Array(size * size * 4)
  for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) {
    const nx = (height(x - 1, y) - height(x + 1, y)) * 1.6, ny = (height(x, y - 1) - height(x, y + 1)) * 1.6, length = Math.hypot(nx, ny, 1), i = (y * size + x) * 4
    data[i] = (nx / length * 0.5 + 0.5) * 255; data[i + 1] = (ny / length * 0.5 + 0.5) * 255; data[i + 2] = (1 / length * 0.5 + 0.5) * 255; data[i + 3] = 255
  }
  const texture = new THREE.DataTexture(data, size, size)
  texture.wrapS = THREE.RepeatWrapping; texture.wrapT = THREE.RepeatWrapping; texture.magFilter = THREE.LinearFilter; texture.minFilter = THREE.LinearMipmapLinearFilter; texture.generateMipmaps = true; texture.needsUpdate = true
  texture.repeat.set(2, 4)
  return texture
}

export interface ExpertLanyardScene { flip: (front: boolean) => void; dispose: () => void }
export function createExpertLanyardScene(container: HTMLElement, card: { frontImage: string; name: string }, onUnavailable: () => void): ExpertLanyardScene {
  const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true, powerPreference: 'low-power' })
  const canvas = renderer.domElement
  const scene = new THREE.Scene()
  const resources = new Set<{ dispose(): void }>()
  const own = <T extends { dispose(): void }>(resource: T): T => { resources.add(resource); return resource }
  const image = new Image()
  let alive = true, raf = 0, target = 0, angle = 0
  let observer: ResizeObserver | undefined
  let themeObserver: MutationObserver | undefined
  let backPainter: ReturnType<typeof createExpertCardBackPainter> | undefined
  let motion: MediaQueryList | undefined
  let onMotionChange: (() => void) | undefined
  const onContextLost = (event: Event) => { event.preventDefault(); dispose(); onUnavailable() }
  const dispose = () => {
    if (!alive) return
    alive = false
    cancelAnimationFrame(raf); observer?.disconnect(); themeObserver?.disconnect()
    backPainter?.dispose()
    if (onMotionChange) motion?.removeEventListener('change', onMotionChange)
    image.onload = null; image.onerror = null; image.removeAttribute('src')
    canvas.removeEventListener('webglcontextlost', onContextLost)
    resources.forEach(resource => resource.dispose()); resources.clear()
    scene.clear(); scene.environment = null
    renderer.dispose(); renderer.forceContextLoss(); canvas.remove()
  }
  try {
    renderer.setClearColor(0x000000, 0)
    renderer.outputColorSpace = THREE.SRGBColorSpace
    renderer.toneMapping = THREE.NeutralToneMapping
    renderer.toneMappingExposure = 1
    canvas.setAttribute('aria-hidden', 'true')
    container.appendChild(canvas)
    scene.environment = own(buildEnvironment(renderer)).texture
    // 128×190 host; 112×150 card. The strap is clipped at the top edge.
    const camera = new THREE.OrthographicCamera(-0.96, 0.96, 1.5, -1.35, 0.1, 100)
    camera.position.set(0, 0, 12); camera.lookAt(0, 0, 0)
    const keyLight = new THREE.DirectionalLight(0xffffff, 1.6), fillLight = new THREE.DirectionalLight(0xffffff, 0.35)
    keyLight.position.set(-2.5, 4, 6); fillLight.position.set(3, -1, 4)
    scene.add(keyLight, fillLight)
    const faceCanvas = document.createElement('canvas')
    faceCanvas.width = 1024; faceCanvas.height = 1372
    const ctx = faceCanvas.getContext('2d')
    if (!ctx) throw new Error('Canvas unavailable')
    ctx.scale(2, 2)
    ctx.fillStyle = '#ffffff'; ctx.fillRect(0, 0, 512, 686)
    const texture = own(new THREE.CanvasTexture(faceCanvas))
    texture.colorSpace = THREE.SRGBColorSpace
    texture.anisotropy = Math.min(8, renderer.capabilities.getMaxAnisotropy())
    const frontMaterial = own(new THREE.MeshBasicMaterial({ map: texture, toneMapped: false }))
    const backCanvas = document.createElement('canvas')
    backCanvas.width = 1024; backCanvas.height = 1372
    paintExpertCardBack(backCanvas, container)
    const backTexture = own(new THREE.CanvasTexture(backCanvas))
    backTexture.colorSpace = THREE.SRGBColorSpace
    backTexture.anisotropy = texture.anisotropy
    const backMaterial = own(new THREE.MeshBasicMaterial({ map: backTexture, toneMapped: false }))
    const edgeMaterial = own(new THREE.MeshPhysicalMaterial({ color: '#ebebeb', roughness: 0.35, clearcoat: 1, clearcoatRoughness: 0.08 }))
    const metal = own(new THREE.MeshStandardMaterial({ color: '#d9dce2', roughness: 0.16, metalness: 1 }))
    const weave = own(buildWeaveTexture())
    const bandMaterial = own(new THREE.MeshPhysicalMaterial({ color: '#111111', normalMap: weave, normalScale: new THREE.Vector2(0.7, 0.7), roughness: 0.68, sheen: 1, sheenRoughness: 0.42, sheenColor: new THREE.Color(0.32, 0.32, 0.34), side: THREE.DoubleSide }))
    const geometries = buildCardGeometry()
    own(geometries.body); own(geometries.face)
    const group = new THREE.Group()
    const body = new THREE.Mesh(geometries.body, edgeMaterial)
    const front = new THREE.Mesh(geometries.face, frontMaterial)
    const back = new THREE.Mesh(own(geometries.face.clone()), backMaterial)
    front.position.z = THICKNESS / 2 + BEVEL + 0.0006
    back.position.z = -front.position.z; back.rotation.y = Math.PI
    const ring = new THREE.Mesh(own(new THREE.TorusGeometry(RING_RADIUS, RING_TUBE, 18, 72)), metal)
    ring.position.y = HEIGHT / 2 - 0.055; ring.rotation.y = Math.PI / 2 - 0.7
    group.add(body, front, back, ring)
    group.position.y = -0.14; group.rotation.y = 0; scene.add(group)
    const hangY = group.position.y + HEIGHT / 2 - 0.055 + RING_RADIUS
    const clamp = new THREE.Mesh(own(buildClampGeometry()), metal)
    const eyelet = new THREE.Mesh(own(new THREE.TorusGeometry(EYELET_RADIUS, 0.0105, 14, 40)), metal)
    eyelet.position.y = hangY
    clamp.position.y = hangY + EYELET_RADIUS + CLAMP_BODY / 2 - 0.004
    scene.add(clamp, eyelet)
    {
      const top = new THREE.Vector2(0, 1.56)
      const bottom = new THREE.Vector2(0, hangY + EYELET_RADIUS + CLAMP_BODY - 0.025)
      const band = new THREE.Mesh(own(new THREE.PlaneGeometry(0.12, top.distanceTo(bottom))), bandMaterial)
      band.position.set((top.x + bottom.x) / 2, (top.y + bottom.y) / 2, -0.025)
      band.rotation.z = -Math.atan2(top.x - bottom.x, top.y - bottom.y)
      scene.add(band)
    }
    const render = () => {
      if (!alive) return
      try { renderer.render(scene, camera) }
      catch { dispose(); onUnavailable() }
    }
    image.crossOrigin = 'anonymous'; image.decoding = 'async'
    const paintFront = () => {
      if (!alive || !image.naturalWidth || !image.naturalHeight) return
      const styles = getComputedStyle(container)
      ctx.fillStyle = styles.getPropertyValue('--fox-expert-card-fill').trim() || '#f5f5f4'
      ctx.fillRect(0, 0, 512, 686)
      const portraitSize = 686 * .5
      const portraitLeft = (512 - portraitSize) / 2
      const scale = Math.max(portraitSize / image.naturalWidth, portraitSize / image.naturalHeight)
      const w = image.naturalWidth * scale, h = image.naturalHeight * scale
      ctx.save(); ctx.beginPath(); ctx.roundRect(portraitLeft, 84, portraitSize, portraitSize, 28); ctx.clip()
      ctx.fillStyle = 'rgba(255,255,255,.36)'; ctx.fillRect(portraitLeft, 84, portraitSize, portraitSize)
      ctx.drawImage(image, (512 - w) / 2, 84 + (portraitSize - h) / 2, w, h); ctx.restore()
      ctx.textAlign = 'center'; ctx.textBaseline = 'alphabetic'
      const textLines = (text: string, limit: number) => {
        const lines: string[] = []; let line = ''
        for (const char of text.replace(/\s+/g, ' ').trim()) {
          if (line && ctx.measureText(line + char).width > 448) { lines.push(line); line = '' }
          line += char
        }
        if (line) lines.push(line)
        if (lines.length > limit) {
          let last = lines[limit - 1]
          while (last && ctx.measureText(last + '…').width > 448) last = last.slice(0, -1)
          lines.splice(limit - 1, lines.length, last + '…')
        }
        return lines
      }
      ctx.fillStyle = styles.getPropertyValue('--fox-expert-card-ink').trim() || '#292c33'
      ctx.font = '600 60px "HarmonyOS Sans SC", sans-serif'
      textLines(card.name, 1).forEach(text => ctx.fillText(text, 256, 535))
      ctx.fillStyle = styles.getPropertyValue('--fox-expert-card-muted').trim() || '#555b66'
      ctx.font = '500 32px "HarmonyOS Sans SC", sans-serif'
      ctx.letterSpacing = '4px'
      ctx.fillText('Fox Agent', 256, 593)
      ctx.letterSpacing = '0px'
      texture.needsUpdate = true; render()
    }
    image.onload = paintFront
    // A remote avatar without CORS can still be displayed by the DOM fallback.
    image.onerror = () => { if (alive) { dispose(); onUnavailable() } }
    image.src = card.frontImage
    backPainter = createExpertCardBackPainter(backCanvas, container, () => {
      if (alive) { backTexture.needsUpdate = true; render() }
    })
    void document.fonts.ready.then(paintFront)
    themeObserver = new MutationObserver(paintFront)
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'data-color-theme'] })
    const resize = () => {
      if (!alive) return
      renderer.setPixelRatio(Math.min(Math.max(window.devicePixelRatio || 1, 2), 3))
      renderer.setSize(Math.max(1, container.clientWidth), Math.max(1, container.clientHeight), false)
      render()
    }
    observer = new ResizeObserver(resize); observer.observe(container)
    motion = window.matchMedia('(prefers-reduced-motion: reduce)')
    onMotionChange = () => {
      if (!alive || !motion?.matches) return
      cancelAnimationFrame(raf); raf = 0
      angle = target; group.rotation.y = angle; render()
    }
    motion.addEventListener('change', onMotionChange)
    canvas.addEventListener('webglcontextlost', onContextLost)
    resize()
    if (!alive) throw new Error('WebGL unavailable')
    return {
      flip(frontSide) {
        cancelAnimationFrame(raf); raf = 0
        if (!alive) return
        target = frontSide ? 0 : Math.PI
        backPainter?.setActive(!frontSide)
        if (motion?.matches) { angle = target; group.rotation.y = angle; render(); return }
        const from = angle, start = performance.now()
        const tick = (now: number) => {
          raf = 0
          if (!alive) return
          const t = Math.min(1, (now - start) / 420), eased = 1 - Math.pow(1 - t, 3)
          angle = from + (target - from) * eased; group.rotation.y = angle; render()
          if (alive && t < 1) raf = requestAnimationFrame(tick)
        }
        raf = requestAnimationFrame(tick)
      },
      dispose,
    }
  } catch (error) { dispose(); throw error }
}
