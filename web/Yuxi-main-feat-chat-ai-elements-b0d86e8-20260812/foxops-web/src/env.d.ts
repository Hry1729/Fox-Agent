/// <reference types="vite/client" />

declare module 'nprogress'

declare module 'crypto-js'

declare module 'vue-img-cutter'

declare module 'file-saver'

declare module '@/components/chat/AgentEditModal.vue' {
  import type { DefineComponent } from 'vue'
  const component: DefineComponent<
    Record<string, never>,
    {
      openCreate: () => void
      openEdit: (agent: unknown) => void
    }
  >
  export default component
}

declare module '@/store/modules/agent' {
  export const useAgentStore: () => any
  export const isBuiltinAgent: (agent: unknown) => boolean
}

declare module 'qrcode.vue' {
  export type Level = 'L' | 'M' | 'Q' | 'H'
  export type RenderAs = 'canvas' | 'svg'
  export type GradientType = 'linear' | 'radial'
  export interface ImageSettings {
    src: string
    height: number
    width: number
    excavate: boolean
  }
  export interface QRCodeProps {
    value: string
    size?: number
    level?: Level
    background?: string
    foreground?: string
    renderAs?: RenderAs
  }
  const QrcodeVue: any
  export default QrcodeVue
}

// 全局变量声明
declare const __APP_VERSION__: string // 版本号
