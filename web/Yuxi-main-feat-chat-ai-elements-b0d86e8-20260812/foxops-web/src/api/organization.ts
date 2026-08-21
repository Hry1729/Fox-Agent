import { request } from './request'

export interface DepartmentItem {
  id: number
  name: string
  description?: string
}

export interface OrgUserItem {
  id?: number
  uid?: string
  username?: string
  userName?: string
  role?: string
  department_id?: number | null
  department_name?: string
}

export const organizationApi = {
  /** 管理员可访问的部门列表 */
  getDepartments() {
    return request.get<DepartmentItem[]>('/api/departments')
  },

  /** 共享范围用的用户选项（与 Yuxi ShareConfigForm 一致） */
  getUserAccessOptions() {
    return request.get<OrgUserItem[]>('/api/auth/users/access-options')
  }
}
