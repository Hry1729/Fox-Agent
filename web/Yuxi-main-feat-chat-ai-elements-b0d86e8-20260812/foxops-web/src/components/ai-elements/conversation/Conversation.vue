<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/utils'
import { reactiveOmit } from '@vueuse/core'
import { StickToBottom } from 'vue-stick-to-bottom'

interface Props {
  ariaLabel?: string
  class?: HTMLAttributes['class']
  initial?: boolean | 'instant' | { damping?: number, stiffness?: number, mass?: number }
  resize?: 'instant' | { damping?: number, stiffness?: number, mass?: number }
  damping?: number
  stiffness?: number
  mass?: number
  anchor?: 'auto' | 'none'
}

const props = withDefaults(defineProps<Props>(), {
  ariaLabel: 'Conversation',
  initial: true,
  damping: 0.7,
  stiffness: 0.05,
  mass: 1.25,
  anchor: 'none',
})
const delegatedProps = reactiveOmit(props, 'class')
</script>

<template>
  <!--
    StickToBottom 根节点不接收 class。外包 + 强制子树高度，
    让内部 overflow:auto 成为真正的滚动容器。
  -->
  <div
    :class="cn('chat-conversation-root relative flex min-h-0 flex-1 flex-col overflow-hidden', props.class)"
    :aria-label="props.ariaLabel"
    role="log"
  >
    <StickToBottom v-bind="delegatedProps" class="chat-conversation-stick">
      <slot />
      <template v-if="$slots.overlay" #overlay="slotProps">
        <slot name="overlay" v-bind="slotProps" />
      </template>
      <template v-if="$slots.after" #after="slotProps">
        <slot name="after" v-bind="slotProps" />
      </template>
    </StickToBottom>
  </div>
</template>

<style scoped>
.chat-conversation-root {
  height: 100%;
}

/* StickToBottom 外层 + 相对定位层 + 滚动层：三条高度链强制打通 */
.chat-conversation-root > :deep(div),
.chat-conversation-root > :deep(div > div) {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  width: 100%;
  height: 100% !important;
  min-height: 0 !important;
  max-height: 100%;
}

/* 真正的滚动节点（StickToBottom 第三层） */
.chat-conversation-root > :deep(div > div > div) {
  flex: 1 1 auto;
  height: 100% !important;
  min-height: 0 !important;
  max-height: 100%;
  overflow-y: auto !important;
  overscroll-behavior: contain;
}
</style>
