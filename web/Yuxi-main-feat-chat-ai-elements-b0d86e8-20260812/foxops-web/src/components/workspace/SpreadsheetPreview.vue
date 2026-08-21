<template>
  <div class="spreadsheet-preview">
    <div v-if="sheets.length > 1" class="sheet-tabs" role="tablist" aria-label="工作表">
      <button
        v-for="(sheet, index) in sheets"
        :key="`${sheet.name}-${index}`"
        type="button"
        role="tab"
        class="sheet-tab"
        :class="{ active: activeIndex === index }"
        :aria-selected="activeIndex === index"
        @click="activeIndex = index"
      >
        {{ sheet.name }}
      </button>
    </div>

    <div v-if="activeSheet" class="sheet-meta">
      <span>{{ activeSheet.totalRows }} 行 × {{ activeSheet.totalColumns }} 列</span>
      <ElTag v-if="activeSheet.truncated" size="small" type="warning" effect="plain">
        仅展示前 {{ workbook?.limits?.rows || 200 }} 行、{{ workbook?.limits?.columns || 50 }} 列
      </ElTag>
    </div>

    <div v-if="activeSheet?.rows?.length" class="sheet-grid">
      <table>
        <tbody>
          <tr v-for="(row, rowIndex) in activeSheet.rows" :key="rowIndex">
            <th class="row-number">{{ rowIndex + 1 }}</th>
            <td v-for="(cell, cellIndex) in row" :key="cellIndex" :title="cell">
              {{ cell }}
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <ElEmpty v-else description="当前工作表没有可显示的数据" />
  </div>
</template>

<script setup lang="ts">
  export interface SpreadsheetSheet {
    name: string
    rows: string[][]
    totalRows: number
    totalColumns: number
    truncated: boolean
  }

  export interface SpreadsheetWorkbook {
    sheets: SpreadsheetSheet[]
    truncated?: boolean
    limits?: { rows: number; columns: number }
  }

  const props = withDefaults(defineProps<{ workbook?: SpreadsheetWorkbook | null }>(), {
    workbook: null
  })
  const activeIndex = ref(0)
  const sheets = computed(() => props.workbook?.sheets || [])
  const activeSheet = computed(() => sheets.value[activeIndex.value] || sheets.value[0] || null)

  watch(
    () => props.workbook,
    () => {
      activeIndex.value = 0
    }
  )
</script>

<style scoped>
  .spreadsheet-preview {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    height: 100%;
    gap: 10px;
  }

  .sheet-tabs {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .sheet-tab {
    max-width: 220px;
    height: 30px;
    padding: 0 12px;
    overflow: hidden;
    border: 1px solid var(--el-border-color);
    border-radius: 6px;
    background: var(--el-bg-color);
    color: var(--el-text-color-regular);
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: pointer;
  }

  .sheet-tab.active {
    border-color: var(--el-color-primary-light-5);
    background: var(--el-color-primary-light-9);
    color: var(--el-color-primary);
  }

  .sheet-meta {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--el-text-color-secondary);
    font-size: 12px;
  }

  .sheet-grid {
    flex: 1;
    min-height: 0;
    overflow: auto;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 6px;
  }

  table {
    min-width: 100%;
    border-collapse: separate;
    border-spacing: 0;
    background: var(--el-bg-color);
    font-size: 12px;
  }

  th,
  td {
    max-width: 320px;
    min-width: 96px;
    height: 32px;
    padding: 5px 8px;
    overflow: hidden;
    border-right: 1px solid var(--el-border-color-lighter);
    border-bottom: 1px solid var(--el-border-color-lighter);
    text-align: left;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row-number {
    position: sticky;
    left: 0;
    z-index: 1;
    min-width: 48px;
    width: 48px;
    background: var(--el-fill-color-light);
    color: var(--el-text-color-secondary);
    text-align: center;
  }

  tr:first-child td {
    background: var(--el-fill-color-lighter);
    font-weight: 600;
  }
</style>
