<template>
  <ExtensionInfoCard
    :title="title"
    :subtitle="metaLine"
    :description="description"
    :icon="icon"
    :brand-icon-url="brandIconUrl"
    :accent="accent"
    :tags="tags"
    :show-arrow="showArrow"
    :disabled="disabled"
    @click="onClick"
  >
    <template v-if="$slots.action" #action>
      <slot name="action" />
    </template>
    <template v-if="$slots.footer" #footer>
      <slot name="footer" />
    </template>
  </ExtensionInfoCard>
</template>

<script setup lang="ts">
  import ExtensionInfoCard, {
    type ExtensionCardTag
  } from '@/components/extensions/common/ExtensionInfoCard.vue'
  import type { KbTypeAccent } from '@/utils/kbUtils'

  export type ResourceCardTag = ExtensionCardTag

  const props = withDefaults(
    defineProps<{
      title: string
      metaLine?: string
      description?: string
      icon?: string
      brandIconUrl?: string
      accent?: KbTypeAccent | 'cyan'
      tags?: ResourceCardTag[]
      disabled?: boolean
      showArrow?: boolean
    }>(),
    {
      metaLine: '',
      description: '暂无描述',
      icon: 'ri:database-2-line',
      brandIconUrl: '',
      accent: 'blue',
      tags: () => [],
      disabled: false,
      showArrow: true
    }
  )

  const emit = defineEmits<{ click: [] }>()

  const onClick = () => {
    if (props.disabled) return
    emit('click')
  }
</script>
