import { useEffect, useRef, type CSSProperties, type MouseEventHandler, type MutableRefObject, type ReactNode, type RefObject } from 'react'
import { Color, Mesh, Program, Renderer, Triangle } from 'ogl'
import './specular-button.css'

const PAD = 20

const vertexWebgl2 = `#version 300 es
in vec2 position;
void main(){gl_Position=vec4(position,0.0,1.0);}
`

const vertexWebgl1 = `
attribute vec2 position;
void main(){gl_Position=vec4(position,0.0,1.0);}
`

const fragmentBody = `
precision highp float;
uniform vec2 uCenter;
uniform vec2 uHalfSize;
uniform float uRadius;
uniform float uAngle;
uniform float uPx;
uniform vec3 uLineColor;
uniform vec3 uBaseColor;
uniform float uIntensity;
uniform float uShineSize;
uniform float uShineFade;
uniform float uThickness;
uniform float uBaseWidth;
uniform float uLengthVariation;
float sdRoundedRect(vec2 p,vec2 b,float r){vec2 q=abs(p)-b+r;return length(max(q,0.0))+min(max(q.x,q.y),0.0)-r;}
float gaussianLine(float d,float sigma){float x=d/(sigma+1e-6);float k=mix(1.0,1.6,smoothstep(0.0,1.5,x));return exp(-k*x*x);}
void renderSpecular(out vec4 color){
  vec2 p=gl_FragCoord.xy-uCenter;
  float d=sdRoundedRect(p,uHalfSize,uRadius);
  vec2 light=vec2(cos(uAngle),sin(uAngle));
  float variationAngle=uAngle*0.58;
  float drift=0.38*sin(variationAngle*0.37+1.7*sin(variationAngle*0.13));
  float shearX=0.18*sin(variationAngle*0.29+0.9*sin(variationAngle*0.61));
  float shearY=0.15*sin(variationAngle*0.43-0.8*cos(variationAngle*0.17));
  float cs=cos(drift),sn=sin(drift);
  vec2 fieldP=vec2(cs*p.x-sn*p.y, sn*p.x+cs*p.y);
  fieldP=vec2(fieldP.x+fieldP.y*shearX,fieldP.y+fieldP.x*shearY);
  vec2 normal=normalize(fieldP/(uHalfSize*uHalfSize)+1e-6);
  float phi=acos(clamp(abs(dot(normal,light)),0.0,1.0));
  float edgePhase=abs(p.x/max(uHalfSize.x,1.0))*5.7+abs(p.y/max(uHalfSize.y,1.0))*7.3;
  float slowNoise=0.5+0.5*sin(variationAngle*0.53+1.3*sin(variationAngle*0.19)+0.7*cos(variationAngle*0.83));
  float detailNoise=0.55*sin(edgePhase+variationAngle*0.71)+0.45*sin(edgePhase*1.83-variationAngle*0.41);
  float dynamicSize=uShineSize*mix(1.0,mix(0.82,0.92,slowNoise),uLengthVariation);
  float dynamicFade=uShineFade*mix(1.0,mix(0.82,0.98,0.5+0.5*sin(variationAngle*0.31+1.2*cos(variationAngle*0.11))),uLengthVariation);
  float warpedPhi=phi+detailNoise*dynamicFade*0.17*uLengthVariation;
  float rim=1.0-smoothstep(dynamicSize-dynamicFade,dynamicSize+dynamicFade+1e-4,warpedPhi);
  float line=gaussianLine(d,uThickness);
  float edgeClamp=1.0-smoothstep(0.5*uPx,3.0*uPx,abs(d));
  float highlight=line*rim*edgeClamp*uIntensity;
  color=vec4(uLineColor*highlight,clamp(highlight,0.0,1.0));
}
`

const fragmentWebgl2 = `#version 300 es
${fragmentBody}
out vec4 fragColor;
void main(){renderSpecular(fragColor);}
`

const fragmentWebgl1 = `
${fragmentBody}
void main(){vec4 color=vec4(0.0);renderSpecular(color);gl_FragColor=color;}
`

type SpecularButtonProps = {
  children?: ReactNode
  size?: 'sm' | 'md' | 'lg'
  radius?: number
  tint?: string
  tintOpacity?: number
  blur?: number
  textColor?: string
  lineColor?: string
  baseColor?: string
  intensity?: number
  shineSize?: number
  shineFade?: number
  thickness?: number
  speed?: number
  followMouse?: boolean
  proximity?: number
  autoAnimate?: boolean
  disabled?: boolean
  onClick?: MouseEventHandler<HTMLButtonElement>
  className?: string
  type?: 'button' | 'submit' | 'reset'
}

type SpecularRenderProps = {
  radius: number
  lineColor: string
  baseColor: string
  intensity: number
  shineSize: number
  shineFade: number
  thickness: number
  speed: number
  followMouse: boolean
  proximity: number
  autoAnimate: boolean
  lengthVariation: number
}

type SpecularStyle = CSSProperties & {
  '--sb-radius': string
  '--sb-tint': string
  '--sb-tint-opacity': number
  '--sb-blur': string
  '--sb-text-color': string
  '--sb-line-color': string
}

type SpecularBorderStyle = CSSProperties & {
  '--sb-radius': string
  '--sb-line-color': string
}

function useSpecularRenderer<T extends HTMLElement>(
  targetRef: RefObject<T | null>,
  effectsRef: RefObject<HTMLSpanElement | null>,
  propsRef: MutableRefObject<SpecularRenderProps>,
) {
  useEffect(() => {
    const target = targetRef.current
    const effects = effectsRef.current
    if (!target || !effects) return

    let renderer: Renderer
    try {
      renderer = new Renderer({ webgl: 2, alpha: true, premultipliedAlpha: true, antialias: true, dpr: Math.min(window.devicePixelRatio || 1, 1.5) })
    } catch {
      return
    }
    const gl = renderer.gl
    gl.clearColor(0, 0, 0, 0)
    gl.enable(gl.BLEND)
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA)
    const geometry = new Triangle(gl)
    if (geometry.attributes.uv) delete geometry.attributes.uv
    const program = new Program(gl, {
      vertex: renderer.isWebgl2 ? vertexWebgl2 : vertexWebgl1,
      fragment: renderer.isWebgl2 ? fragmentWebgl2 : fragmentWebgl1,
      transparent: true, depthTest: false, depthWrite: false, cullFace: false,
      uniforms: {
        uCenter: { value: [0, 0] }, uHalfSize: { value: [1, 1] }, uRadius: { value: 0 },
        uAngle: { value: 2.4 }, uPx: { value: renderer.dpr }, uLineColor: { value: [1, 1, 1] },
        uBaseColor: { value: [.32, .32, .32] }, uIntensity: { value: 1 },
        uShineSize: { value: .17 }, uShineFade: { value: .7 }, uThickness: { value: 1 },
        uBaseWidth: { value: renderer.dpr }, uLengthVariation: { value: 1 },
      },
    })
    program.setBlendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA)
    if (!gl.getProgramParameter(program.program, gl.LINK_STATUS)) {
      geometry.remove(); program.remove(); gl.getExtension('WEBGL_lose_context')?.loseContext(); return
    }
    const mesh = new Mesh(gl, { geometry, program })
    effects.appendChild(gl.canvas)
    target.dataset.specularRenderer = renderer.isWebgl2 ? 'webgl2' : 'webgl1'

    const sizeRef = { width: 1, height: 1 }
    const resize = () => {
      const rect = target.getBoundingClientRect()
      sizeRef.width = rect.width; sizeRef.height = rect.height
      renderer.setSize(rect.width + PAD * 2, rect.height + PAD * 2)
      const dpr = renderer.dpr
      program.uniforms.uCenter.value = [(PAD + rect.width / 2) * dpr, (PAD + rect.height / 2) * dpr]
      program.uniforms.uHalfSize.value = [(rect.width / 2) * dpr, (rect.height / 2) * dpr]
    }
    const resizeObserver = new ResizeObserver(resize)
    resizeObserver.observe(target)
    resize()

    let pointerAngle: number | null = null
    let proximityValue = 0
    const onPointerMove = (event: PointerEvent) => {
      const rect = target.getBoundingClientRect()
      const centerX = rect.left + rect.width / 2
      const centerY = rect.top + rect.height / 2
      const dx = Math.max(rect.left - event.clientX, 0, event.clientX - rect.right)
      const dy = Math.max(rect.top - event.clientY, 0, event.clientY - rect.bottom)
      const distance = Math.hypot(dx, dy)
      pointerAngle = distance === 0
        ? Math.atan2(2 / rect.height, -2 / rect.width) + ((event.clientX - centerX) / (rect.width / 2)) * .3 + ((centerY - event.clientY) / (rect.height / 2)) * .15
        : Math.atan2(centerY - event.clientY, event.clientX - centerX)
      const value = Math.max(0, 1 - distance / Math.max(propsRef.current.proximity, 1))
      proximityValue = value * value * (3 - 2 * value)
    }
    window.addEventListener('pointermove', onPointerMove)

    let angle = 2.4
    let idleAngle = 2.4
    let brightness = 0
    let previous = performance.now()
    let animationFrame = 0
    let pageVisible = !document.hidden
    let elementVisible = false
    const line = new Color()
    const base = new Color()
    const update = (now: number) => {
      animationFrame = 0
      if (!pageVisible || !elementVisible) return
      const delta = Math.min((now - previous) / 1000, .05)
      previous = now
      const current = propsRef.current
      idleAngle += current.speed * delta
      const steer = current.followMouse && pointerAngle !== null && (!current.autoAnimate || proximityValue > 0)
      const targetAngle = steer ? pointerAngle! : idleAngle
      const difference = ((targetAngle - angle + Math.PI * 3) % (Math.PI * 2)) - Math.PI
      angle += difference * (1 - Math.exp(-delta * 7))
      const brightnessTarget = current.autoAnimate ? 1 : proximityValue
      brightness += (brightnessTarget - brightness) * (1 - Math.exp(-delta * 8))
      line.set(current.lineColor); base.set(current.baseColor)
      program.uniforms.uAngle.value = angle
      program.uniforms.uRadius.value = Math.min(current.radius, Math.min(sizeRef.width, sizeRef.height) / 2) * renderer.dpr
      program.uniforms.uLineColor.value = [line.r, line.g, line.b]
      program.uniforms.uBaseColor.value = [base.r, base.g, base.b]
      program.uniforms.uIntensity.value = current.intensity * brightness
      program.uniforms.uShineSize.value = current.shineSize * Math.PI / 180
      program.uniforms.uShineFade.value = current.shineFade * Math.PI / 180
      program.uniforms.uThickness.value = current.thickness * renderer.dpr
      program.uniforms.uLengthVariation.value = current.lengthVariation
      renderer.render({ scene: mesh })
      animationFrame = window.requestAnimationFrame(update)
    }
    const start = () => {
      if (!pageVisible || !elementVisible || animationFrame !== 0) return
      previous = performance.now()
      animationFrame = window.requestAnimationFrame(update)
    }
    const stop = () => {
      if (animationFrame !== 0) window.cancelAnimationFrame(animationFrame)
      animationFrame = 0
    }
    const observer = new IntersectionObserver(([entry]) => {
      elementVisible = entry?.isIntersecting ?? false
      elementVisible ? start() : stop()
    })
    observer.observe(target)
    const onVisibilityChange = () => {
      pageVisible = !document.hidden
      pageVisible ? start() : stop()
    }
    document.addEventListener('visibilitychange', onVisibilityChange)
    const onContextLost = (event: Event) => {
      event.preventDefault(); stop(); gl.canvas.style.display = 'none'; delete target.dataset.specularRenderer
    }
    gl.canvas.addEventListener('webglcontextlost', onContextLost)

    return () => {
      stop()
      observer.disconnect()
      resizeObserver.disconnect()
      document.removeEventListener('visibilitychange', onVisibilityChange)
      window.removeEventListener('pointermove', onPointerMove)
      gl.canvas.removeEventListener('webglcontextlost', onContextLost)
      delete target.dataset.specularRenderer
      gl.deleteShader(program.vertexShader); gl.deleteShader(program.fragmentShader)
      geometry.remove(); program.remove(); gl.canvas.remove()
      gl.getExtension('WEBGL_lose_context')?.loseContext()
    }
  }, [effectsRef, propsRef, targetRef])
}

export function SpecularButton({
  children = 'Get Started', size = 'lg', radius = 18, tint = '#ffffff', tintOpacity = 0,
  blur = 0, textColor = '#f5f5f5', lineColor = '#ffffff', baseColor = '#525252',
  intensity = 1, shineSize = 10, shineFade = 40, thickness = 1, speed = .35,
  followMouse = true, proximity = 250, autoAnimate = false, disabled = false,
  onClick, className = '', type = 'button',
}: SpecularButtonProps) {
  const buttonRef = useRef<HTMLButtonElement>(null)
  const effectsRef = useRef<HTMLSpanElement>(null)
  const propsRef = useRef({ radius, lineColor, baseColor, intensity, shineSize, shineFade, thickness, speed, followMouse, proximity, autoAnimate, lengthVariation: 1 })
  propsRef.current = { radius, lineColor, baseColor, intensity, shineSize, shineFade, thickness, speed, followMouse, proximity, autoAnimate, lengthVariation: 1 }
  useSpecularRenderer(buttonRef, effectsRef, propsRef)

  const style: SpecularStyle = {
    '--sb-radius': `${radius}px`, '--sb-tint': tint, '--sb-tint-opacity': tintOpacity,
    '--sb-blur': `${blur}px`, '--sb-text-color': textColor, '--sb-line-color': lineColor,
  }
  return <button ref={buttonRef} type={type} disabled={disabled} onClick={onClick} className={`specular-button specular-button--${size}${className ? ` ${className}` : ''}`} style={style}>
    <span ref={effectsRef} className="specular-button__fx" aria-hidden="true" />
    <span className="specular-button__label">{children}</span>
  </button>
}

export type SpecularBorderProps = Partial<SpecularRenderProps> & { className?: string }

export function SpecularBorder({
  className = '', radius = 18, lineColor = '#ffffff', baseColor = '#525252', intensity = 1,
  shineSize = 10, shineFade = 40, thickness = 1, speed = .35, followMouse = false,
  proximity = 250, autoAnimate = true, lengthVariation = 1,
}: SpecularBorderProps) {
  const borderRef = useRef<HTMLSpanElement>(null)
  const effectsRef = useRef<HTMLSpanElement>(null)
  const propsRef = useRef({ radius, lineColor, baseColor, intensity, shineSize, shineFade, thickness, speed, followMouse, proximity, autoAnimate, lengthVariation })
  propsRef.current = { radius, lineColor, baseColor, intensity, shineSize, shineFade, thickness, speed, followMouse, proximity, autoAnimate, lengthVariation }
  useSpecularRenderer(borderRef, effectsRef, propsRef)

  return <span ref={borderRef} className={`specular-border${className ? ` ${className}` : ''}`} style={{ '--sb-radius': `${radius}px`, '--sb-line-color': lineColor } as SpecularBorderStyle} aria-hidden="true">
    <span ref={effectsRef} className="specular-border__fx" />
  </span>
}
