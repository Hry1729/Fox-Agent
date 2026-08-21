<template>
  <span
    class="fallback-avatar"
    :class="[`fallback-avatar--${shape}`, { 'fallback-avatar--image': currentSrc }]"
    :style="[avatarSizeStyle, fallbackStyle]"
  >
    <img
      v-if="currentSrc"
      :key="currentSrc"
      class="fallback-avatar-image"
      :src="currentSrc"
      :alt="resolvedAlt"
      @error="handleImageError"
    />
    <span v-else class="fallback-avatar-text" aria-hidden="true">{{ initials }}</span>
  </span>
</template>

<script setup lang="ts">
  import { getAvatarFallbackStyle, getAvatarInitials } from '@/utils/pixelAvatar'

  const props = withDefaults(
    defineProps<{
      src?: string
      defaultSrc?: string
      name?: string
      seed?: string | number
      kind?: 'user' | 'agent'
      size?: string | number
      shape?: 'circle' | 'rounded'
      alt?: string
    }>(),
    {
      src: '',
      defaultSrc: '',
      name: '',
      seed: '',
      kind: 'user',
      size: 32,
      shape: 'circle',
      alt: ''
    }
  )

  const failedImageCount = ref(0)

  const imageCandidates = computed(() => {
    const candidates = [props.src, props.defaultSrc]
      .map((value) => String(value || '').trim())
      .filter((value) => {
        if (!value) return false
        if (value === '/' || value === '#' || value === 'about:blank') return false
        if (value === 'undefined' || value === 'null') return false
        if (/^https?:\/\/[^/]+\/?$/i.test(value)) return false
        return true
      })
    return [...new Set(candidates)]
  })

  const currentSrc = computed(() => imageCandidates.value[failedImageCount.value] || '')

  const initials = computed(() => getAvatarInitials(props.name, props.kind))

  const fallbackStyle = computed(() =>
    getAvatarFallbackStyle(props.seed || props.name || props.kind)
  )

  const avatarSizeStyle = computed(() => {
    const size = typeof props.size === 'number' ? `${props.size}px` : props.size
    const fontSize =
      typeof props.size === 'number' ? `${Math.max(10, Math.floor(props.size * 0.34))}px` : '12px'
    return {
      '--fallback-avatar-size': size,
      '--fallback-avatar-font-size': fontSize
    }
  })

  const resolvedAlt = computed(() => props.alt || `${props.name || '头像'}图标`)

  watch(
    () => [props.src, props.defaultSrc],
    () => {
      failedImageCount.value = 0
    }
  )

  const handleImageError = () => {
    if (failedImageCount.value < imageCandidates.value.length - 1) {
      failedImageCount.value += 1
    } else {
      failedImageCount.value = imageCandidates.value.length
    }
  }
</script>

<style scoped>
  .fallback-avatar {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: var(--fallback-avatar-size);
    height: var(--fallback-avatar-size);
    overflow: hidden;
    flex-shrink: 0;
  }

  .fallback-avatar--circle {
    border-radius: 50%;
  }

  .fallback-avatar--rounded {
    border-radius: 10px;
  }

  .fallback-avatar-image {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .fallback-avatar-text {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
    font-size: var(--fallback-avatar-font-size);
    font-weight: 600;
    line-height: 1;
  }
</style>
