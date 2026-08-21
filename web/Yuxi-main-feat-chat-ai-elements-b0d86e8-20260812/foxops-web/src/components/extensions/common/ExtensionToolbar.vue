<template>
  <div class="extension-toolbar">
    <ElRow justify="space-between" :gutter="10" class="toolbar-row">
      <ElCol :lg="6" :md="8" :sm="14" :xs="24">
        <ElInput
          v-model="searchProxy"
          clearable
          :placeholder="searchPlaceholder"
          @keyup.enter="$emit('refresh')"
        >
          <template #prefix>
            <ArtSvgIcon icon="ri:search-line" />
          </template>
        </ElInput>
      </ElCol>
      <ElCol :lg="10" :md="8" :sm="0" :xs="0" class="toolbar-filters-col">
        <div class="toolbar-filters">
          <slot name="filters" />
        </div>
      </ElCol>
      <ElCol :lg="8" :md="8" :sm="10" :xs="24" class="toolbar-actions-col">
        <div class="toolbar-actions">
          <slot name="actions" />
          <ElButton v-if="showRefresh" :loading="loading" @click="$emit('refresh')">
            <ArtSvgIcon icon="ri:refresh-line" />
          </ElButton>
        </div>
      </ElCol>
    </ElRow>
  </div>
</template>

<script setup lang="ts">
  const props = withDefaults(
    defineProps<{
      search?: string
      searchPlaceholder?: string
      loading?: boolean
      showRefresh?: boolean
    }>(),
    { search: '', searchPlaceholder: '搜索...', loading: false, showRefresh: true }
  )
  const emit = defineEmits<{ 'update:search': [value: string]; refresh: [] }>()
  const searchProxy = computed({
    get: () => props.search,
    set: (value: string) => emit('update:search', value)
  })
</script>

<style scoped>
  .extension-toolbar {
    margin-bottom: 20px;
  }
  .toolbar-row {
    width: 100%;
  }
  .toolbar-filters-col {
    display: flex;
    align-items: center;
  }
  .toolbar-filters,
  .toolbar-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .toolbar-actions-col {
    display: flex;
    justify-content: flex-end;
  }
  @media (max-width: 768px) {
    .toolbar-actions-col {
      justify-content: flex-start;
      margin-top: 10px;
    }
  }
</style>
