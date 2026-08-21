<template>
  <span class="file-type-icon" :class="`is-${kind}`" aria-hidden="true">
    <ArtSvgIcon :icon="icon" />
  </span>
</template>

<script setup lang="ts">
  const props = withDefaults(
    defineProps<{ name?: string; isDir?: boolean; folderVariant?: string }>(),
    { name: '', isDir: false, folderVariant: 'default' }
  )

  const extension = computed(() => {
    const filename = props.name.toLowerCase().split(/[?#]/)[0].split('/').pop() || ''
    return filename.includes('.') ? filename.split('.').pop() || '' : ''
  })

  const kind = computed(() => {
    if (props.isDir) return 'folder'
    if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp'].includes(extension.value))
      return 'image'
    if (extension.value === 'pdf') return 'pdf'
    if (['md', 'markdown', 'mdx'].includes(extension.value)) return 'markdown'
    if (['doc', 'docx'].includes(extension.value)) return 'word'
    if (['xls', 'xlsx', 'csv'].includes(extension.value)) return 'sheet'
    if (['ppt', 'pptx'].includes(extension.value)) return 'slides'
    if (['zip', 'rar', '7z', 'tar', 'gz'].includes(extension.value)) return 'archive'
    if (['mp3', 'wav', 'flac', 'mp4', 'mov', 'webm'].includes(extension.value)) return 'media'
    if (
      ['py', 'js', 'ts', 'vue', 'json', 'yaml', 'yml', 'css', 'less', 'html', 'sql'].includes(
        extension.value
      )
    )
      return 'code'
    return 'file'
  })

  // Remix Icon 无 folder-star；Saved Artifacts 用 folder-open 表达收藏/快捷目录
  const folderIcons: Record<string, string> = {
    personal: 'ri:folder-user-line',
    favorite: 'ri:folder-open-line',
    agent: 'ri:folder-settings-line',
    knowledge: 'ri:folder-chart-line',
    enterprise: 'ri:folder-shared-line',
    default: 'ri:folder-3-line'
  }
  const fileIcons: Record<string, string> = {
    image: 'ri:image-line',
    pdf: 'ri:file-pdf-2-line',
    markdown: 'ri:markdown-line',
    word: 'ri:file-word-2-line',
    sheet: 'ri:file-excel-2-line',
    slides: 'ri:file-ppt-2-line',
    archive: 'ri:file-zip-line',
    media: 'ri:file-music-line',
    code: 'ri:file-code-line',
    file: 'ri:file-3-line'
  }
  const icon = computed(() =>
    props.isDir ? folderIcons[props.folderVariant] || folderIcons.default : fileIcons[kind.value]
  )
</script>

<style scoped>
  .file-type-icon {
    display: inline-flex;
    flex: 0 0 auto;
    font-size: 18px;
    color: var(--el-text-color-secondary);
  }
  .is-folder {
    color: #e5a33d;
  }
  .is-image {
    color: #8b5cf6;
  }
  .is-pdf {
    color: #ef4444;
  }
  .is-markdown,
  .is-code {
    color: #3b82f6;
  }
  .is-word {
    color: #2563eb;
  }
  .is-sheet {
    color: #16a34a;
  }
  .is-slides {
    color: #ea580c;
  }
</style>
