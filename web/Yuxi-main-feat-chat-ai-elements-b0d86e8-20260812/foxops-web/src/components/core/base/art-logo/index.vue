<!-- 系统logo -->
<template>
  <div class="flex-cc">
    <img
      :style="logoStyle"
      :src="collapsed ? miniLogo : fullLogo"
      alt="SIPG"
      class="h-full object-contain"
    />
  </div>
</template>

<script setup lang="ts">
  import fullLogo from '@imgs/logo.png'
  import miniLogo from '@imgs/logo-mini.png'

  defineOptions({ name: 'ArtLogo' })

  interface Props {
    /** logo 高度 */
    size?: number | string
    /** 折叠态使用方形 SIPG 标识 */
    collapsed?: boolean
  }

  const props = withDefaults(defineProps<Props>(), {
    size: 36,
    collapsed: false
  })

  const logoStyle = computed(() => ({ height: `${props.size}px` }))
</script>
