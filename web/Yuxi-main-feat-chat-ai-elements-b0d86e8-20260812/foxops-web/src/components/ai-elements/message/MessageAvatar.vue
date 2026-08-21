<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar'
import { usableMediaSrc } from '@/utils/mediaSrc'
import { computed } from 'vue'

interface Props {
  src?: string
  name?: string
  class?: HTMLAttributes['class']
}

const props = withDefaults(defineProps<Props>(), {
  src: '',
  name: '',
})

const fallbackText = computed(() => props.name?.slice(0, 2) || 'ME')
const imageSrc = computed(() => usableMediaSrc(props.src))
</script>

<template>
  <Avatar class="size-8 ring-1 ring-border" :class="[props.class]" v-bind="$attrs">
    <AvatarImage v-if="imageSrc" alt="" class="mt-0 mb-0" :src="imageSrc" />
    <AvatarFallback>{{ fallbackText }}</AvatarFallback>
  </Avatar>
</template>
