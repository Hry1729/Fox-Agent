<template>
  <div class="mcp-env-editor">
    <div v-for="(row, index) in rows" :key="index" class="env-row">
      <ElInput v-model="row.key" placeholder="键" @change="emitValue" />
      <ElInput v-model="row.value" placeholder="值" show-password @change="emitValue" />
      <ElButton text type="danger" @click="removeRow(index)">
        <ArtSvgIcon icon="ri:delete-bin-line" />
      </ElButton>
    </div>
    <ElButton @click="addRow">添加环境变量</ElButton>
  </div>
</template>

<script setup lang="ts">
  const props = defineProps<{ modelValue?: Record<string, string> | null }>()
  const emit = defineEmits<{ 'update:modelValue': [value: Record<string, string> | null] }>()

  const rows = ref<Array<{ key: string; value: string }>>([])

  const syncFromProps = () => {
    const entries = Object.entries(props.modelValue || {})
    rows.value = entries.length
      ? entries.map(([key, value]) => ({ key, value: String(value ?? '') }))
      : []
  }

  const emitValue = () => {
    const result: Record<string, string> = {}
    const seen = new Set<string>()
    for (const row of rows.value) {
      const key = row.key.trim()
      if (!key) continue
      if (seen.has(key)) continue
      seen.add(key)
      result[key] = row.value
    }
    emit('update:modelValue', Object.keys(result).length ? result : null)
  }

  const addRow = () => {
    rows.value.push({ key: '', value: '' })
  }

  const removeRow = (index: number) => {
    rows.value.splice(index, 1)
    emitValue()
  }

  watch(() => props.modelValue, syncFromProps, { immediate: true, deep: true })
</script>

<style scoped>
  .mcp-env-editor {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .env-row {
    display: grid;
    grid-template-columns: 1fr 1fr auto;
    gap: 6px;
  }
</style>
