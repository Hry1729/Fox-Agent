// UI 1102：界面上的日期时间统一用 zh-CN、24 小时制，不跟随系统区域。
// 侧栏、消息页脚、通知等展示层都从这里取格式，避免出现「12:31 AM」「11/01/2026」。

const LOCALE = 'zh-CN'

const timeFormat = new Intl.DateTimeFormat(LOCALE, { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' })
const dateTimeFormat = new Intl.DateTimeFormat(LOCALE, {
  year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23'
})
const shortDateTimeFormat = new Intl.DateTimeFormat(LOCALE, {
  month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit', hourCycle: 'h23'
})

function toDate(value: Date | string | number): Date | null {
  const date = value instanceof Date ? value : new Date(value)
  return Number.isNaN(date.getTime()) ? null : date
}

/** 「14:05」 */
export function formatClockTime(value: Date | string | number): string {
  const date = toDate(value)
  return date ? timeFormat.format(date) : ''
}

/** 「2026/11/01 14:05」 */
export function formatDateTime(value: Date | string | number): string {
  const date = toDate(value)
  return date ? dateTimeFormat.format(date) : ''
}

/** 「11月1日 14:05」 */
export function formatShortDateTime(value: Date | string | number): string {
  const date = toDate(value)
  return date ? shortDateTimeFormat.format(date) : ''
}
