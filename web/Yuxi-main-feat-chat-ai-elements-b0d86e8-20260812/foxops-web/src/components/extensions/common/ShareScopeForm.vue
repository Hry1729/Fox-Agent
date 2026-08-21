<template>
  <div class="share-scope-form" :class="{ 'is-cards': variant === 'cards', 'is-readonly': readonly }">
    <template v-if="variant === 'cards'">
      <div
        class="share-mode-cards"
        :class="`active-${accessLevel}`"
        role="radiogroup"
        aria-label="共享设置"
      >
        <div
          v-for="option in visibleCardOptions"
          :key="option.value"
          role="radio"
          class="share-mode-card"
          :class="{ active: accessLevel === option.value }"
          :aria-checked="accessLevel === option.value"
          :tabindex="!readonly && accessLevel === option.value ? 0 : -1"
          @click="setAccessLevel(option.value)"
          @keydown.enter.prevent="setAccessLevel(option.value)"
          @keydown.space.prevent="setAccessLevel(option.value)"
        >
          <div class="card-main">
            <div class="card-header">
              <div class="card-icon-wrapper" aria-hidden="true">
                <ArtSvgIcon :icon="option.icon" class="card-icon" />
              </div>
              <div class="card-title">{{ option.title }}</div>
              <div
                v-if="accessLevel === option.value && option.value !== 'global'"
                class="card-action"
                @click.stop
              >
                <ElPopover
                  trigger="click"
                  placement="bottom-end"
                  :width="280"
                  popper-class="share-selection-popover"
                >
                  <template #reference>
                    <ElButton
                      size="small"
                      class="select-action"
                      :aria-label="option.value === 'department' ? '选择部门' : '选择用户'"
                      :disabled="readonly"
                    >
                      <ArtSvgIcon icon="ri:user-add-line" class="select-action-icon" />
                      <span class="access-count">{{ getAccessCount(option.value) }}</span>
                    </ElButton>
                  </template>
                  <div class="selection-dropdown" @mousedown.stop @click.stop>
                    <div class="selection-dropdown-header">
                      <div class="selection-dropdown-title">
                        {{ option.value === 'department' ? '可访问部门' : '可访问用户' }}
                      </div>
                      <div class="selection-dropdown-subtitle">
                        {{ getAccessSummary(option.value) }}
                      </div>
                    </div>
                    <ElInput
                      v-model="selectionSearch[option.value]"
                      size="small"
                      clearable
                      class="selection-search"
                      :placeholder="option.value === 'department' ? '搜索部门' : '搜索用户'"
                      @mousedown.stop
                      @click.stop
                    />
                    <div v-if="getSelectionOptions(option.value).length" class="selection-list">
                      <div
                        v-for="item in getSelectionOptions(option.value)"
                        :key="item.value"
                        role="checkbox"
                        :aria-checked="isSelected(option.value, item.value)"
                        :tabindex="item.disabled ? -1 : 0"
                        class="selection-item"
                        :class="{
                          selected: isSelected(option.value, item.value),
                          locked: item.disabled
                        }"
                        @mousedown.stop
                        @click.stop="
                          !item.disabled &&
                          toggleSelection(
                            option.value,
                            item.value,
                            !isSelected(option.value, item.value)
                          )
                        "
                      >
                        <span class="selection-item-content">
                          <ElCheckbox
                            :model-value="isSelected(option.value, item.value)"
                            :disabled="item.disabled || readonly"
                            @click.stop
                            @change="
                              (checked) =>
                                toggleSelection(option.value, item.value, Boolean(checked))
                            "
                          />
                          <span class="selection-label">{{ item.label }}</span>
                        </span>
                        <span v-if="item.disabled" class="selection-required">必选</span>
                      </div>
                    </div>
                    <div v-else class="selection-empty">暂无可选项</div>
                  </div>
                </ElPopover>
              </div>
            </div>
            <div class="card-description">{{ option.description }}</div>
          </div>
        </div>
      </div>
    </template>

    <template v-else>
      <ElRadioGroup v-model="accessLevel" :disabled="readonly" @change="onLevelChange">
        <ElRadio v-for="option in visibleLevels" :key="option.value" :value="option.value">
          {{ option.label }}
        </ElRadio>
      </ElRadioGroup>

      <div v-if="accessLevel === 'department'" class="scope-picker">
        <ElSelect
          v-model="departmentIds"
          multiple
          filterable
          :disabled="readonly"
          placeholder="选择可访问部门"
          style="width: 100%"
          @change="emitChange"
        >
          <ElOption v-for="item in departments" :key="item.id" :label="item.name" :value="item.id" />
        </ElSelect>
      </div>

      <div v-if="accessLevel === 'user'" class="scope-picker">
        <ElSelect
          v-model="userUids"
          multiple
          filterable
          :disabled="readonly"
          placeholder="选择可访问用户"
          style="width: 100%"
          @change="emitChange"
        >
          <ElOption
            v-for="item in users"
            :key="item.uid || item.id"
            :label="
              item.department_name
                ? `${item.username || item.userName}（${item.department_name}）`
                : item.username || item.userName || item.uid || String(item.id)
            "
            :value="String(item.uid || item.id)"
          />
        </ElSelect>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
  import { organizationApi, type DepartmentItem, type OrgUserItem } from '@/api/organization'
  import { useUserStore } from '@/store/modules/user'
  import { unwrapApiData, unwrapList } from '@/utils/apiData'

  export interface ShareConfig {
    access_level?: string
    department_ids?: number[]
    user_uids?: string[]
    /** 兼容旧字段 */
    level?: string
    user_ids?: number[]
  }

  const props = withDefaults(
    defineProps<{
      modelValue?: ShareConfig | null
      readonly?: boolean
      allowedAccessLevels?: string[]
      variant?: 'radio' | 'cards'
      autoSelectUserDept?: boolean
    }>(),
    {
      modelValue: () => ({ access_level: 'global' }),
      readonly: false,
      allowedAccessLevels: () => ['global', 'department', 'user'],
      variant: 'radio',
      autoSelectUserDept: true
    }
  )
  const emit = defineEmits<{ 'update:modelValue': [value: ShareConfig] }>()
  const userStore = useUserStore()

  const levelOptions = [
    { value: 'global', label: '全局可见' },
    { value: 'department', label: '指定部门可见' },
    { value: 'user', label: '指定用户可见' }
  ]

  const cardOptions = [
    {
      value: 'global',
      title: '全局共享',
      description: '所有用户都可以访问',
      icon: 'ri:global-line'
    },
    {
      value: 'department',
      title: '部门共享',
      description: '选中的部门成员可以访问',
      icon: 'ri:building-line'
    },
    {
      value: 'user',
      title: '指定人',
      description: '选中的用户可以访问',
      icon: 'ri:group-line'
    }
  ]

  const visibleLevels = computed(() =>
    levelOptions.filter((item) => props.allowedAccessLevels.includes(item.value))
  )

  const visibleCardOptions = computed(() =>
    cardOptions.filter((item) => props.allowedAccessLevels.includes(item.value))
  )

  const selectionSearch = reactive<Record<string, string>>({
    department: '',
    user: ''
  })

  const currentDepartmentId = computed(() => {
    const deptId = Number(
      (userStore.info as any)?.departmentId || (userStore.info as any)?.department_id
    )
    return Number.isFinite(deptId) && deptId > 0 ? deptId : null
  })

  const currentUserUid = computed(() => String(userStore.info?.uid || ''))

  const normalizeLevel = (value?: ShareConfig | null) => {
    const raw = value?.access_level || value?.level || 'global'
    if (raw === 'departments') return 'department'
    if (raw === 'users') return 'user'
    return props.allowedAccessLevels.includes(raw) ? raw : props.allowedAccessLevels[0] || 'global'
  }

  const accessLevel = ref(normalizeLevel(props.modelValue))
  const departmentIds = ref<number[]>([...(props.modelValue?.department_ids || [])])
  const userUids = ref<string[]>((props.modelValue?.user_uids || []).map(String).filter(Boolean))
  const departments = ref<DepartmentItem[]>([])
  const users = ref<OrgUserItem[]>([])

  const ensureCurrentDepartment = () => {
    if (!props.autoSelectUserDept || !currentDepartmentId.value) return
    if (!departmentIds.value.includes(currentDepartmentId.value)) {
      departmentIds.value = [currentDepartmentId.value, ...departmentIds.value]
    }
  }

  const ensureCurrentUser = () => {
    if (!currentUserUid.value) return
    if (!userUids.value.includes(currentUserUid.value)) {
      userUids.value = [currentUserUid.value, ...userUids.value]
    }
  }

  const emitChange = () => {
    emit('update:modelValue', {
      access_level: accessLevel.value,
      department_ids: accessLevel.value === 'department' ? [...departmentIds.value] : [],
      user_uids: accessLevel.value === 'user' ? [...userUids.value] : []
    })
  }

  const onLevelChange = () => {
    if (accessLevel.value === 'department') {
      ensureCurrentDepartment()
      userUids.value = []
    } else if (accessLevel.value === 'user') {
      ensureCurrentUser()
      departmentIds.value = []
    } else {
      departmentIds.value = []
      userUids.value = []
    }
    emitChange()
  }

  const setAccessLevel = (level: string) => {
    if (props.readonly || !props.allowedAccessLevels.includes(level)) return
    if (accessLevel.value === level) return
    accessLevel.value = level
    onLevelChange()
  }

  const departmentOptions = computed(() =>
    departments.value.map((dept) => {
      const value = Number(dept.id)
      return {
        label: dept.name,
        value,
        disabled: value === currentDepartmentId.value
      }
    })
  )

  const userOptions = computed(() =>
    users.value.map((user) => ({
      label: user.department_name
        ? `${user.username || user.userName}（${user.department_name}）`
        : user.username || user.userName || String(user.uid || user.id),
      value: String(user.uid || user.id),
      disabled: String(user.uid || user.id) === currentUserUid.value
    }))
  )

  const getAccessSummary = (level: string) => {
    if (level === 'global') return '所有用户可访问'
    if (level === 'department') return `${departmentIds.value.length} 个部门可访问`
    if (level === 'user' && userUids.value.length === 1) return '仅自己可访问'
    return `${userUids.value.length} 个用户可访问`
  }

  const getAccessCount = (level: string) => {
    if (level === 'department') return departmentIds.value.length
    if (level === 'user') return userUids.value.length
    return ''
  }

  const getSelectionOptions = (level: string) => {
    const options = level === 'department' ? departmentOptions.value : userOptions.value
    const query = selectionSearch[level as 'department' | 'user']?.trim().toLowerCase()
    if (!query) return options
    return options.filter((item) => item.label.toLowerCase().includes(query))
  }

  const isSelected = (level: string, value: string | number) => {
    if (level === 'department') return departmentIds.value.includes(Number(value))
    if (level === 'user') return userUids.value.includes(String(value))
    return false
  }

  const toggleSelection = (level: string, value: string | number, checked: boolean) => {
    if (props.readonly) return
    if (level === 'department') {
      const departmentId = Number(value)
      departmentIds.value = checked
        ? [...new Set([...departmentIds.value, departmentId])]
        : departmentIds.value.filter((id) => id !== departmentId)
      ensureCurrentDepartment()
    } else {
      const uid = String(value)
      userUids.value = checked
        ? [...new Set([...userUids.value, uid])]
        : userUids.value.filter((item) => item !== uid)
      ensureCurrentUser()
    }
    emitChange()
  }

  watch(
    () => props.modelValue,
    (value) => {
      accessLevel.value = normalizeLevel(value)
      departmentIds.value = [...(value?.department_ids || [])]
      userUids.value = (value?.user_uids || []).map(String).filter(Boolean)
    },
    { deep: true }
  )

  watch(currentDepartmentId, () => {
    if (accessLevel.value === 'department') ensureCurrentDepartment()
  })

  watch(currentUserUid, () => {
    if (accessLevel.value === 'user') ensureCurrentUser()
  })

  onMounted(async () => {
    try {
      const [deptRes, userRes] = await Promise.all([
        organizationApi.getDepartments(),
        organizationApi.getUserAccessOptions()
      ])
      departments.value = unwrapList(deptRes)
      if (!departments.value.length) {
        const nested = unwrapApiData<any>(deptRes)
        departments.value = nested?.departments || []
      }
      users.value = unwrapList(userRes)
      if (!users.value.length) {
        const nested = unwrapApiData<any>(userRes)
        users.value = nested?.users || nested || []
      }
      if (accessLevel.value === 'department') ensureCurrentDepartment()
      if (accessLevel.value === 'user') ensureCurrentUser()
    } catch {
      departments.value = []
      users.value = []
    }
  })

  defineExpose({
    validate: () => {
      if (accessLevel.value === 'global') return { valid: true, message: '' }
      if (accessLevel.value === 'department') {
        if (!currentDepartmentId.value) {
          return { valid: false, message: '您不属于任何部门，无法使用部门共享模式' }
        }
        if (!departmentIds.value.includes(currentDepartmentId.value)) {
          return { valid: false, message: '您所在的部门必须在可访问部门范围内' }
        }
        return { valid: true, message: '' }
      }
      if (!currentUserUid.value) {
        return { valid: false, message: '无法获取当前用户，无法使用指定人可访问模式' }
      }
      if (!userUids.value.includes(currentUserUid.value)) {
        return { valid: false, message: '当前用户必须在可访问用户范围内' }
      }
      return { valid: true, message: '' }
    }
  })
</script>

<style scoped>
  .share-scope-form {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .scope-picker {
    margin-top: 0;
  }

  .share-scope-form.is-cards {
    width: 100%;
  }

  .share-mode-cards {
    display: grid;
    width: 100%;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 8px;
    align-items: stretch;
  }

  .share-mode-card {
    position: relative;
    display: flex;
    min-width: 0;
    min-height: 76px;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
    border: 1px solid var(--el-border-color);
    border-radius: 12px;
    background: var(--el-bg-color);
    cursor: pointer;
    transition:
      border-color 0.18s ease,
      background-color 0.18s ease,
      box-shadow 0.18s ease;
  }

  .share-mode-card:hover,
  .share-mode-card:focus-visible {
    border-color: var(--el-color-primary);
  }

  .share-mode-card:focus-visible {
    outline: none;
    box-shadow: 0 0 0 3px var(--el-color-primary-light-7);
  }

  .share-mode-card.active {
    border-color: var(--el-color-primary);
    background: linear-gradient(180deg, var(--el-color-primary-light-9) 0%, var(--el-bg-color) 100%);
    box-shadow:
      0 0 0 1px var(--el-color-primary-light-8),
      0 5px 12px rgb(0 0 0 / 6%);
  }

  .is-readonly .share-mode-card {
    cursor: not-allowed;
    opacity: 0.78;
  }

  .card-main {
    display: flex;
    min-width: 0;
    flex: 1;
    flex-direction: column;
    gap: 8px;
  }

  .card-header {
    display: flex;
    align-items: center;
    min-width: 0;
    gap: 10px;
  }

  .card-action {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-left: auto;
  }

  .card-icon-wrapper {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    flex-shrink: 0;
    border-radius: 10px;
    background: var(--el-fill-color-light);
    transition: background-color 0.18s ease;
  }

  .card-icon {
    font-size: 20px;
    color: var(--el-text-color-secondary);
  }

  .share-mode-card.active .card-icon-wrapper {
    background: var(--el-color-primary-light-9);
  }

  .share-mode-card.active .card-icon {
    color: var(--el-color-primary);
  }

  .card-title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    font-size: 14px;
    font-weight: 600;
    color: var(--el-text-color-primary);
    line-height: 1.35;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .card-description {
    font-size: 12px;
    line-height: 1.45;
    color: var(--el-text-color-secondary);
  }

  .access-count {
    color: var(--el-color-primary);
    font-size: 12px;
    font-weight: 500;
    line-height: 1;
  }

  .select-action {
    min-width: 44px;
    height: 24px;
    padding: 0 8px;
  }

  .select-action-icon {
    font-size: 14px;
  }

  .selection-dropdown {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .selection-dropdown-header {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .selection-dropdown-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .selection-dropdown-subtitle {
    font-size: 12px;
    color: var(--el-text-color-secondary);
  }

  .selection-list {
    display: flex;
    max-height: 220px;
    flex-direction: column;
    gap: 4px;
    overflow-y: auto;
  }

  .selection-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 6px 8px;
    border-radius: 6px;
    cursor: pointer;
  }

  .selection-item:hover,
  .selection-item.selected {
    background: var(--el-fill-color-light);
  }

  .selection-item.locked {
    cursor: default;
  }

  .selection-item-content {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .selection-label {
    font-size: 13px;
    color: var(--el-text-color-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .selection-required {
    flex-shrink: 0;
    font-size: 11px;
    color: var(--el-text-color-secondary);
  }

  .selection-empty {
    padding: 12px 0;
    text-align: center;
    font-size: 12px;
    color: var(--el-text-color-secondary);
  }

  @media (max-width: 768px) {
    .share-mode-cards {
      grid-template-columns: 1fr;
    }
  }
</style>

<style>
  .share-selection-popover.el-popover.el-popper {
    padding: 12px;
  }
</style>
