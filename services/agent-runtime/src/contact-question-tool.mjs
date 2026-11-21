import { Type } from 'typebox'
import { defineFoxTool } from './tool-adapter.mjs'

export const CONTACT_QUESTION_TOOL = 'ask_user'

const questionText = Type.String({ minLength: 1, maxLength: 300 })
const questionOptions = Type.Array(Type.String({ minLength: 1, maxLength: 160 }), { minItems: 2, maxItems: 8 })

function toCardQuestion(input, index) {
  const question = input.question.trim()
  const options = (input.options ?? []).map(option => option.trim())
  if (!question || options.some(option => !option) || new Set(options).size !== options.length) {
    throw new Error('请填写清楚的问题，并确保每个选项非空且不重复。')
  }
  return {
    question_id: `q-${index + 1}`,
    question,
    options: options.map(label => ({ label, value: label })),
    multi_select: input.multi_select === true,
    allow_other: true,
  }
}

export function createUserQuestionTool(onQuestion) {
  return defineFoxTool({
    name: CONTACT_QUESTION_TOOL,
    label: '向用户提问',
    description: '在问题卡中一次询问会影响当前交付的必要信息。每项给出有意义的选项，也可省略选项让用户填写。单独调用并等待答复，不在正文重复提问。',
    parameters: Type.Object({
      questions: Type.Array(Type.Object({
        question: questionText,
        options: Type.Optional(questionOptions),
        multi_select: Type.Optional(Type.Boolean()),
      }), { minItems: 1, maxItems: 3 }),
    }),
    execute(_callId, { questions }) {
      onQuestion({ questions: questions.map(toCardQuestion) })
      return {
        content: [{ type: 'text', text: '已显示问题卡，等待用户回答。' }],
        terminate: true,
      }
    },
  })
}

export function createContactQuestionTool(onQuestion) {
  return defineFoxTool({
    name: CONTACT_QUESTION_TOOL,
    label: '请用户选择',
    description: '只有用户需要在会影响交付的方案之间做决定时才用。说明写在 explanation，Fox 会先发送聊天气泡，再显示选项卡。请单独调用此工具并等用户答复。',
    parameters: Type.Object({
      explanation: Type.String({ minLength: 1, maxLength: 800 }),
      question: questionText,
      options: questionOptions,
    }),
    execute(_callId, input) {
      const explanation = input.explanation.trim()
      if (!explanation) throw new Error('请提供情况说明。')
      onQuestion({
        explanation,
        questions: [toCardQuestion(input, 0)],
      })
      return {
        content: [{ type: 'text', text: '已发出情况说明和选项卡，等待用户回答。' }],
        terminate: true,
      }
    },
  })
}
