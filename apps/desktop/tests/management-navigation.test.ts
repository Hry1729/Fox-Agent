import { describe, expect, test } from 'bun:test'
import {
  captureManagementReturnRoutes,
  managementExitRoute,
  type ManagementReturnRoutes,
  type WorkspaceReturnRoute,
} from '../src/features/workspace/management-navigation'

const chatRoute: WorkspaceReturnRoute = { view: 'chat', entityId: null, documentId: null }

function initialRoutes(): ManagementReturnRoutes {
  return { settings: chatRoute, knowledge: chatRoute }
}

describe('management navigation', () => {
  test('exits settings to the main page that opened it', () => {
    const knowledgeRoute: WorkspaceReturnRoute = { view: 'knowledge', entityId: null, documentId: null }
    const routes = captureManagementReturnRoutes(initialRoutes(), knowledgeRoute, 'login')

    expect(managementExitRoute('login', routes)).toEqual(knowledgeRoute)
    expect(managementExitRoute('service', routes)).toEqual(knowledgeRoute)
  })

  test('keeps knowledge return history when settings opens inside knowledge', () => {
    const knowledgeRoute: WorkspaceReturnRoute = { view: 'knowledge', entityId: null, documentId: null }
    const enteredKnowledge = captureManagementReturnRoutes(initialRoutes(), chatRoute, 'knowledge')
    const enteredSettings = captureManagementReturnRoutes(enteredKnowledge, knowledgeRoute, 'login')
    const returnedToKnowledge = captureManagementReturnRoutes(enteredSettings, { ...knowledgeRoute, view: 'login' }, 'knowledge')

    expect(managementExitRoute('login', enteredSettings)).toEqual(knowledgeRoute)
    expect(managementExitRoute('knowledge', returnedToKnowledge)).toEqual(chatRoute)
  })

  test('does not overwrite the settings origin within settings', () => {
    const enteredSettings = captureManagementReturnRoutes(initialRoutes(), chatRoute, 'settings')
    const openedLogin = captureManagementReturnRoutes(enteredSettings, { view: 'settings-yuxi', entityId: null, documentId: null }, 'login')

    expect(managementExitRoute('login', openedLogin)).toEqual(chatRoute)
  })
})
