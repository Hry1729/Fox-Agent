<template>
  <KnowledgeResourceCard
    :title="title"
    :meta-line="metaLine"
    :description="description"
    :icon="typeIcon"
    :brand-icon-url="brandIconUrl"
    :accent="accent"
    :tags="tags"
    @click="$emit('click')"
  />
</template>

<script setup lang="ts">
  import type { KnowledgeDatabase, KnowledgeTypeInfo } from '@/api/knowledge'
  import type { ResourceCardTag } from '@/components/extensions/knowledge/KnowledgeResourceCard.vue'
  import KnowledgeResourceCard from '@/components/extensions/knowledge/KnowledgeResourceCard.vue'
  import { formatExtensionCardTitle } from '@/utils/extensionDisplayName'
  import {
    getKbBrandIconUrl,
    getKbCardMeta,
    getKbTypeAccent,
    getKbTypeIcon,
    getKbTypeLabel,
    getEmbeddingModelShortName,
    normalizeKbType
  } from '@/utils/kbUtils'

  const props = withDefaults(
    defineProps<{
      database: KnowledgeDatabase
      types?: Record<string, KnowledgeTypeInfo>
    }>(),
    { types: () => ({}) }
  )

  defineEmits<{ click: [] }>()

  const kbType = computed(() => normalizeKbType(props.database.kb_type))
  const accent = computed(() => getKbTypeAccent(kbType.value))
  const typeIcon = computed(() => getKbTypeIcon(kbType.value))
  const brandIconUrl = computed(() => getKbBrandIconUrl(kbType.value))
  const title = computed(() => formatExtensionCardTitle(props.database.name || props.database.kb_id))
  const metaLine = computed(() => getKbCardMeta(props.database, props.types))
  const description = computed(() => props.database.description?.trim() || '暂无描述')

  const tags = computed<ResourceCardTag[]>(() => {
    const result: ResourceCardTag[] = [
      { name: getKbTypeLabel(kbType.value, props.types), variant: 'type' }
    ]
    const embedding = getEmbeddingModelShortName(props.database)
    if (embedding) result.push({ name: embedding, variant: 'embed' })

    const level = String((props.database.share_config as any)?.access_level || '').trim()
    if (level && level !== 'global') {
      const map: Record<string, string> = {
        department: '部门共享',
        user: '指定用户',
        private: '私有'
      }
      result.push({ name: map[level] || `共享:${level}`, variant: 'share' })
    }
    return result
  })
</script>
