import { AppRouteRecord } from '@/types/router'

/**
 * 智能体扩展：侧栏一级「智能体扩展」+ 二级菜单切换主内容
 * 布局对齐 article 模块（父级 Layout + 子路由页面）
 */
export const extensionsRoutes: AppRouteRecord = {
  path: '/extensions',
  name: 'Extensions',
  component: '/index/index',
  meta: {
    title: '智能体扩展',
    icon: 'ri:puzzle-2-line'
  },
  children: [
    {
      path: 'knowledge',
      name: 'ExtensionKnowledge',
      component: '/extensions/knowledge/index',
      meta: {
        title: '知识库',
        icon: 'ri:database-2-line',
        keepAlive: false,
        roles: ['R_ADMIN', 'R_SUPER']
      }
    },
    {
      path: 'tools',
      name: 'ExtensionTools',
      component: '/extensions/tools/index',
      meta: {
        title: '工具',
        icon: 'ri:tools-line',
        keepAlive: false,
        roles: ['R_ADMIN', 'R_SUPER']
      }
    },
    {
      path: 'mcp',
      name: 'ExtensionMcp',
      component: '/extensions/mcp/index',
      meta: {
        title: 'MCP',
        icon: 'ri:plug-line',
        keepAlive: false,
        roles: ['R_ADMIN', 'R_SUPER']
      }
    },
    {
      path: 'skills',
      name: 'ExtensionSkills',
      component: '/extensions/skills/index',
      meta: {
        title: 'Skills',
        icon: 'ri:magic-line',
        keepAlive: false
      }
    },
    {
      path: 'knowledgebase/:kbId',
      name: 'ExtensionKnowledgeBaseDetail',
      component: '/extensions/knowledge/detail',
      meta: {
        title: '知识库详情',
        isHide: true,
        isHideTab: true,
        roles: ['R_ADMIN', 'R_SUPER'],
        activePath: '/extensions/knowledge'
      }
    },
    {
      path: 'mcp/:slug',
      name: 'ExtensionMcpDetail',
      component: '/extensions/mcp/detail',
      meta: {
        title: 'MCP 详情',
        isHide: true,
        isHideTab: true,
        roles: ['R_ADMIN', 'R_SUPER'],
        activePath: '/extensions/mcp'
      }
    },
    {
      path: 'skill/:slug',
      name: 'ExtensionSkillDetail',
      component: '/extensions/skill/detail',
      meta: {
        title: 'Skill 详情',
        isHide: true,
        isHideTab: true,
        activePath: '/extensions/skills'
      }
    }
  ]
}
