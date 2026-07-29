import { useEffect, useRef } from 'react'
import { Mesh, Program, Renderer, Triangle } from 'ogl'

type GrainientProps = {
  className?: string
  color1?: string
  color2?: string
  color3?: string
  timeSpeed?: number
  colorBalance?: number
  warpStrength?: number
  warpFrequency?: number
  warpSpeed?: number
  warpAmplitude?: number
  blendAngle?: number
  blendSoftness?: number
  rotationAmount?: number
  noiseScale?: number
  grainAmount?: number
  grainScale?: number
  grainAnimated?: boolean
  contrast?: number
  gamma?: number
  saturation?: number
  centerX?: number
  centerY?: number
  zoom?: number
  dpr?: number
}

type GrainientContext = {
  renderer: Renderer
  program: Program
  mesh: Mesh
  geometry: Triangle
}

const contexts = new WeakMap<HTMLDivElement, GrainientContext>()

function hexToRgb(hex: string) {
  const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex)
  if (!result) return [1, 1, 1]
  return [
    Number.parseInt(result[1], 16) / 255,
    Number.parseInt(result[2], 16) / 255,
    Number.parseInt(result[3], 16) / 255,
  ]
}

const vertexWebgl2 = `#version 300 es
in vec2 position;
void main() {
  gl_Position = vec4(position, 0.0, 1.0);
}
`

const vertexWebgl1 = `
attribute vec2 position;
void main() {
  gl_Position = vec4(position, 0.0, 1.0);
}
`

const fragmentBody = `
precision highp float;
uniform vec2 iResolution;
uniform float iTime;
uniform float uTimeSpeed;
uniform float uColorBalance;
uniform float uWarpStrength;
uniform float uWarpFrequency;
uniform float uWarpSpeed;
uniform float uWarpAmplitude;
uniform float uBlendAngle;
uniform float uBlendSoftness;
uniform float uRotationAmount;
uniform float uNoiseScale;
uniform float uGrainAmount;
uniform float uGrainScale;
uniform float uGrainAnimated;
uniform float uContrast;
uniform float uGamma;
uniform float uSaturation;
uniform vec2 uCenterOffset;
uniform float uZoom;
uniform vec3 uColor1;
uniform vec3 uColor2;
uniform vec3 uColor3;
#define S(a,b,t) smoothstep(a,b,t)
mat2 Rot(float a){float s=sin(a),c=cos(a);return mat2(c,-s,s,c);}
vec2 hash(vec2 p){p=vec2(dot(p,vec2(2127.1,81.17)),dot(p,vec2(1269.5,283.37)));return fract(sin(p)*43758.5453);}
float noise(vec2 p){vec2 i=floor(p),f=fract(p),u=f*f*(3.0-2.0*f);float n=mix(mix(dot(-1.0+2.0*hash(i),f),dot(-1.0+2.0*hash(i+vec2(1.0,0.0)),f-vec2(1.0,0.0)),u.x),mix(dot(-1.0+2.0*hash(i+vec2(0.0,1.0)),f-vec2(0.0,1.0)),dot(-1.0+2.0*hash(i+vec2(1.0)),f-vec2(1.0)),u.x),u.y);return 0.5+0.5*n;}
void mainImage(out vec4 o, vec2 C){
  float t=iTime*uTimeSpeed;
  vec2 uv=C/iResolution.xy;
  float ratio=iResolution.x/iResolution.y;
  vec2 tuv=uv-0.5+uCenterOffset;
  tuv/=max(uZoom,0.001);
  float degree=noise(vec2(t*0.1,tuv.x*tuv.y)*uNoiseScale);
  tuv.y*=1.0/ratio;
  tuv*=Rot(radians((degree-0.5)*uRotationAmount+180.0));
  tuv.y*=ratio;
  float frequency=uWarpFrequency;
  float ws=max(uWarpStrength,0.001);
  float amplitude=uWarpAmplitude/ws;
  float warpTime=t*uWarpSpeed;
  tuv.x+=sin(tuv.y*frequency+warpTime)/amplitude;
  tuv.y+=sin(tuv.x*(frequency*1.5)+warpTime)/(amplitude*0.5);
  float b=uColorBalance;
  float s=max(uBlendSoftness,0.0);
  float blendX=(tuv*Rot(radians(uBlendAngle))).x;
  float edge0=-0.3-b-s;
  float edge1=0.2-b+s;
  vec3 layer1=mix(uColor3,uColor2,S(edge0,edge1,blendX));
  vec3 layer2=mix(uColor2,uColor1,S(edge0,edge1,blendX));
  float verticalBlend=1.0-S(-0.3-b-s,0.5-b+s,tuv.y);
  vec3 col=mix(layer1,layer2,verticalBlend);
  vec2 grainUv=uv*max(uGrainScale,0.001);
  if(uGrainAnimated>0.5) grainUv+=vec2(iTime*0.05);
  float grain=fract(sin(dot(grainUv,vec2(12.9898,78.233)))*43758.5453);
  col+=(grain-0.5)*uGrainAmount;
  col=(col-0.5)*uContrast+0.5;
  float luma=dot(col,vec3(0.2126,0.7152,0.0722));
  col=mix(vec3(luma),col,uSaturation);
  col=pow(max(col,0.0),vec3(1.0/max(uGamma,0.001)));
  o=vec4(clamp(col,0.0,1.0),1.0);
}
`

const fragmentWebgl2 = `#version 300 es
${fragmentBody}
out vec4 fragColor;
void main(){vec4 color=vec4(0.0);mainImage(color,gl_FragCoord.xy);fragColor=color;}
`

const fragmentWebgl1 = `
${fragmentBody}
void main(){vec4 color=vec4(0.0);mainImage(color,gl_FragCoord.xy);gl_FragColor=color;}
`

export function Grainient({
  className = '',
  color1 = '#ffffff',
  color2 = '#eaf5ff',
  color3 = '#b8d8f3',
  timeSpeed = 0.5,
  colorBalance = 0,
  warpStrength = 0.95,
  warpFrequency = 5,
  warpSpeed = 1.8,
  warpAmplitude = 52,
  blendAngle = 12,
  blendSoftness = 0.08,
  rotationAmount = 420,
  noiseScale = 1.8,
  grainAmount = 0.035,
  grainScale = 1.8,
  grainAnimated = false,
  contrast = 1.08,
  gamma = 1,
  saturation = 0.9,
  centerX = 0,
  centerY = 0,
  zoom = 0.92,
  dpr = 1.25,
}: GrainientProps) {
  const containerRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const container = containerRef.current
    if (!container) return
    let context: GrainientContext
    try {
      const renderer = new Renderer({ webgl: 2, alpha: true, depth: false, antialias: false, dpr: Math.min(dpr, 1.5) })
      const gl = renderer.gl
      const geometry = new Triangle(gl)
      const program = new Program(gl, {
        vertex: renderer.isWebgl2 ? vertexWebgl2 : vertexWebgl1,
        fragment: renderer.isWebgl2 ? fragmentWebgl2 : fragmentWebgl1,
        depthTest: false,
        depthWrite: false,
        cullFace: false,
        uniforms: {
          iTime: { value: 0 },
          iResolution: { value: new Float32Array([1, 1]) },
          uTimeSpeed: { value: timeSpeed },
          uColorBalance: { value: colorBalance },
          uWarpStrength: { value: warpStrength },
          uWarpFrequency: { value: warpFrequency },
          uWarpSpeed: { value: warpSpeed },
          uWarpAmplitude: { value: warpAmplitude },
          uBlendAngle: { value: blendAngle },
          uBlendSoftness: { value: blendSoftness },
          uRotationAmount: { value: rotationAmount },
          uNoiseScale: { value: noiseScale },
          uGrainAmount: { value: grainAmount },
          uGrainScale: { value: grainScale },
          uGrainAnimated: { value: grainAnimated ? 1 : 0 },
          uContrast: { value: contrast },
          uGamma: { value: gamma },
          uSaturation: { value: saturation },
          uCenterOffset: { value: new Float32Array([centerX, centerY]) },
          uZoom: { value: zoom },
          uColor1: { value: new Float32Array(hexToRgb(color1)) },
          uColor2: { value: new Float32Array(hexToRgb(color2)) },
          uColor3: { value: new Float32Array(hexToRgb(color3)) },
        },
      })
      if (!gl.getProgramParameter(program.program, gl.LINK_STATUS)) {
        gl.deleteShader(program.vertexShader)
        gl.deleteShader(program.fragmentShader)
        program.remove()
        geometry.remove()
        gl.getExtension('WEBGL_lose_context')?.loseContext()
        return
      }
      const mesh = new Mesh(gl, { geometry, program })
      context = { renderer, program, mesh, geometry }
      contexts.set(container, context)
      container.dataset.grainientRenderer = renderer.isWebgl2 ? 'webgl2' : 'webgl1'
      gl.canvas.setAttribute('aria-hidden', 'true')
      container.appendChild(gl.canvas)
    } catch {
      return
    }

    const { renderer, program, mesh, geometry } = context
    const gl = renderer.gl
    const onContextLost = (event: Event) => {
      event.preventDefault()
      stop()
      gl.canvas.style.display = 'none'
      container.dataset.grainientRenderer = 'fallback'
    }
    gl.canvas.addEventListener('webglcontextlost', onContextLost)
    const resize = () => {
      const rect = container.getBoundingClientRect()
      renderer.setSize(Math.max(1, Math.floor(rect.width)), Math.max(1, Math.floor(rect.height)))
      const resolution = program.uniforms.iResolution.value as Float32Array
      resolution[0] = gl.drawingBufferWidth
      resolution[1] = gl.drawingBufferHeight
      renderer.render({ scene: mesh })
    }
    const resizeObserver = new ResizeObserver(resize)
    resizeObserver.observe(container)
    resize()

    let animationFrame = 0
    let lastRenderedAt = 0
    let pageVisible = !document.hidden
    let elementVisible = true
    const startedAt = performance.now()
    const render = (time: number) => {
      if (time - lastRenderedAt < 1000 / 30) {
        animationFrame = window.requestAnimationFrame(render)
        return
      }
      lastRenderedAt = time
      program.uniforms.iTime.value = (time - startedAt) * 0.001
      renderer.render({ scene: mesh })
      animationFrame = window.requestAnimationFrame(render)
    }
    const start = () => {
      if (pageVisible && elementVisible && animationFrame === 0) animationFrame = window.requestAnimationFrame(render)
    }
    const stop = () => {
      if (animationFrame !== 0) window.cancelAnimationFrame(animationFrame)
      animationFrame = 0
    }
    const observer = new IntersectionObserver(([entry]) => {
      elementVisible = entry?.isIntersecting ?? false
      elementVisible ? start() : stop()
    })
    observer.observe(container)
    const onVisibilityChange = () => {
      pageVisible = !document.hidden
      pageVisible ? start() : stop()
    }
    document.addEventListener('visibilitychange', onVisibilityChange)
    start()

    return () => {
      stop()
      observer.disconnect()
      resizeObserver.disconnect()
      document.removeEventListener('visibilitychange', onVisibilityChange)
      gl.canvas.removeEventListener('webglcontextlost', onContextLost)
      contexts.delete(container)
      delete container.dataset.grainientRenderer
      gl.deleteShader(program.vertexShader)
      gl.deleteShader(program.fragmentShader)
      geometry.remove()
      program.remove()
      gl.canvas.remove()
      gl.getExtension('WEBGL_lose_context')?.loseContext()
    }
  }, [])

  useEffect(() => {
    const container = containerRef.current
    const context = container ? contexts.get(container) : undefined
    if (!context) return
    const uniforms = context.program.uniforms
    uniforms.uTimeSpeed.value = timeSpeed
    uniforms.uColorBalance.value = colorBalance
    uniforms.uWarpStrength.value = warpStrength
    uniforms.uWarpFrequency.value = warpFrequency
    uniforms.uWarpSpeed.value = warpSpeed
    uniforms.uWarpAmplitude.value = warpAmplitude
    uniforms.uBlendAngle.value = blendAngle
    uniforms.uBlendSoftness.value = blendSoftness
    uniforms.uRotationAmount.value = rotationAmount
    uniforms.uNoiseScale.value = noiseScale
    uniforms.uGrainAmount.value = grainAmount
    uniforms.uGrainScale.value = grainScale
    uniforms.uGrainAnimated.value = grainAnimated ? 1 : 0
    uniforms.uContrast.value = contrast
    uniforms.uGamma.value = gamma
    uniforms.uSaturation.value = saturation
    uniforms.uCenterOffset.value = new Float32Array([centerX, centerY])
    uniforms.uZoom.value = zoom
    uniforms.uColor1.value = new Float32Array(hexToRgb(color1))
    uniforms.uColor2.value = new Float32Array(hexToRgb(color2))
    uniforms.uColor3.value = new Float32Array(hexToRgb(color3))
  }, [blendAngle, blendSoftness, centerX, centerY, color1, color2, color3, colorBalance, contrast, gamma, grainAmount, grainAnimated, grainScale, noiseScale, rotationAmount, saturation, timeSpeed, warpAmplitude, warpFrequency, warpSpeed, warpStrength, zoom])

  return <div ref={containerRef} className={`grainient-container ${className}`.trim()} aria-hidden="true" />
}
