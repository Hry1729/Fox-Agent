<template>
  <aside class="workspace-sidebar">
    <button
      class="workspace-nav-item"
      :class="{ active: activeKey === 'personal' && !isQuickAccessPath(currentPath) }"
      type="button"
      @click="$emit('select-personal')"
    >
      <FileTypeIcon is-dir folder-variant="personal" />
      <span>个人工作区</span>
    </button>

    <section class="sidebar-section">
      <div class="section-title">快速访问</div>
      <button
        v-for="item in quickAccess"
        :key="item.path"
        class="workspace-nav-item secondary"
        :class="{ active: activeKey === 'personal' && isSameOrChildPath(currentPath, item.path) }"
        type="button"
        @click="$emit('select-path', item.path)"
      >
        <FileTypeIcon is-dir :folder-variant="item.variant" />
        <span>{{ item.label }}</span>
      </button>
    </section>

    <section v-if="myDatabases.length" class="sidebar-section">
      <div class="section-title">我的知识库</div>
      <DatabaseItem
        v-for="database in myDatabases"
        :key="database.kb_id"
        :database="database"
        :active="activeKey === `database:${database.kb_id}`"
        variant="knowledge"
        @select="$emit('select-database', database)"
      />
    </section>

    <section v-if="sharedDatabases.length" class="sidebar-section">
      <div class="section-title">共享知识库</div>
      <DatabaseItem
        v-for="database in sharedDatabases"
        :key="database.kb_id"
        :database="database"
        :active="activeKey === `database:${database.kb_id}`"
        variant="enterprise"
        @select="$emit('select-database', database)"
      />
    </section>

    <div v-if="loadingDatabases" class="sidebar-muted">正在加载知识库...</div>
    <div v-else-if="databaseError" class="sidebar-error">
      <span>知识库连接失败</span>
      <button type="button" @click="$emit('retry-databases')">重试</button>
    </div>
    <div v-else-if="!databases.length" class="sidebar-muted">暂无可访问知识库</div>
  </aside>
</template>

<script setup lang="ts">
  import { defineComponent, h, type PropType } from 'vue'
  import type { AccessibleKnowledgeBase } from '@/api/knowledge'
  import FileTypeIcon from './FileTypeIcon.vue'

  const props = withDefaults(
    defineProps<{
      activeKey?: string
      currentPath?: string
      databases?: AccessibleKnowledgeBase[]
      loadingDatabases?: boolean
      databaseError?: string
      currentUid?: string
    }>(),
    {
      activeKey: 'personal',
      currentPath: '/',
      databases: () => [],
      loadingDatabases: false,
      databaseError: '',
      currentUid: ''
    }
  )
  defineEmits<{
    'select-personal': []
    'select-path': [path: string]
    'select-database': [database: AccessibleKnowledgeBase]
    'retry-databases': []
  }>()

  const quickAccess = [
    { label: 'Saved Artifacts', path: '/saved_artifacts', variant: 'favorite' },
    { label: 'Agents', path: '/agents/', variant: 'agent' }
  ]
  const normalize = (path: string) => String(path || '/').replace(/\/$/, '') || '/'
  const isSameOrChildPath = (path: string, target: string) => {
    const current = normalize(path)
    const parent = normalize(target)
    return current === parent || current.startsWith(`${parent}/`)
  }
  const isQuickAccessPath = (path: string) =>
    quickAccess.some((item) => isSameOrChildPath(path, item.path))
  const myDatabases = computed(() =>
    props.databases.filter(
      (item) => String(item.created_by || '') === String(props.currentUid || '')
    )
  )
  const sharedDatabases = computed(() =>
    props.databases.filter(
      (item) => String(item.created_by || '') !== String(props.currentUid || '')
    )
  )

  const DatabaseItem = defineComponent({
    props: {
      database: { type: Object as PropType<AccessibleKnowledgeBase>, required: true },
      active: Boolean,
      variant: { type: String, default: 'knowledge' }
    },
    emits: ['select'],
    setup(itemProps, { emit }) {
      return () =>
        h(
          'button',
          {
            type: 'button',
            class: ['workspace-nav-item', 'secondary', { active: itemProps.active }],
            onClick: () => emit('select')
          },
          [
            h(FileTypeIcon, { isDir: true, folderVariant: itemProps.variant }),
            h('span', { title: itemProps.database.name }, itemProps.database.name)
          ]
        )
    }
  })
</script>

<style scoped>
  .workspace-sidebar {
    height: 100%;
    padding: 14px 10px;
    border-right: 1px solid var(--el-border-color-lighter);
    overflow-y: auto;
    background: var(--el-bg-color);
  }
  .sidebar-section {
    margin-top: 10px;
  }
  .section-title {
    padding: 4px 10px;
    color: var(--el-text-color-secondary);
    font-size: 12px;
    font-weight: 600;
  }
  .workspace-nav-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    min-height: 36px;
    padding: 0 10px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--el-text-color-regular);
    text-align: left;
    cursor: pointer;
  }
  .workspace-nav-item.secondary {
    min-height: 32px;
    font-size: 13px;
  }
  .workspace-nav-item:hover,
  .workspace-nav-item.active {
    color: var(--el-color-primary);
    background: var(--el-color-primary-light-9);
  }
  .workspace-nav-item span:last-child {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sidebar-muted {
    padding: 12px 10px;
    color: var(--el-text-color-secondary);
    font-size: 12px;
  }

  .sidebar-error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-top: 10px;
    padding: 8px 10px;
    border: 1px solid var(--el-color-danger-light-7);
    border-radius: 8px;
    background: var(--el-color-danger-light-9);
    color: var(--el-color-danger);
    font-size: 12px;
  }

  .sidebar-error button {
    padding: 0;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    font-weight: 600;
    cursor: pointer;
  }
</style>
