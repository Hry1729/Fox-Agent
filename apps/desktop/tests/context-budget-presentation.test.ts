import { describe, expect, test } from 'bun:test'
import {
  calibrationNote,
  compactTokenCount,
  contextBudgetRows,
  estimateSourceLabel,
  estimateSourceMeasured,
  pressurePercent,
  triggerReasonLabel,
  type ContextBudgetDto,
} from '../src/features/chat/components/context-budget-presentation'

function dto(overrides: Partial<ContextBudgetDto> = {}): ContextBudgetDto {
  return {
    modelWindowTokens: 8192,
    systemTokens: 400,
    toolsTokens: 900,
    historyTokens: 2200,
    pendingAppendTokens: 500,
    outputReserveTokens: 512,
    protocolOverheadTokens: 2048,
    safetyMarginTokens: 1000,
    availableTokens: 632,
    estimateSource: 'heuristic',
    pressureRatio: 0.85,
    triggerReason: 'threshold',
    withinBudget: true,
    updatedAt: 123,
    ...overrides,
  }
}

describe('context budget presentation', () => {
  test('itemizes every component exactly once with measured/estimated tags', () => {
    const rows = contextBudgetRows(dto())
    expect(rows.map(row => row.key)).toEqual(['system', 'tools', 'history', 'append', 'reserve', 'overhead', 'margin'])
    const total = rows.reduce((sum, row) => sum + row.tokens, 0) + dto().availableTokens
    expect(total).toBe(8192)
    expect(rows.find(row => row.key === 'reserve')?.exact).toBe(true)
    expect(rows.find(row => row.key === 'history')?.exact).toBe(false)
  })

  test('never presents an estimate as a measurement', () => {
    expect(estimateSourceMeasured(dto())).toBe(false)
    expect(estimateSourceLabel(dto())).toContain('估算')
    const calibrated = dto({ estimateSource: 'provider_usage', usageInputTokens: 9600, calibrationRatioPpm: 1_250_000 })
    expect(estimateSourceMeasured(calibrated)).toBe(true)
    expect(estimateSourceLabel(calibrated)).toContain('实测')
    expect(calibrationNote(calibrated)).toContain('×1.25')
    const mixed = dto({ estimateSource: 'mixed', usageInputTokens: 7000 })
    expect(estimateSourceMeasured(mixed)).toBe(false)
    expect(estimateSourceLabel(mixed)).toContain('估算')
  })

  test('explains trigger reasons including bounded stops', () => {
    expect(triggerReasonLabel(dto({ triggerReason: 'threshold' }))).toContain('压缩')
    expect(triggerReasonLabel(dto({ triggerReason: 'single_item_too_large' }))).toContain('单条')
    expect(triggerReasonLabel(dto({ triggerReason: 'no_reduction' }))).toContain('有界停止')
    expect(triggerReasonLabel(dto({ triggerReason: 'no_candidates' }))).toContain('有界停止')
    expect(triggerReasonLabel(dto({ triggerReason: 'summary_failed' }))).toContain('失败')
    expect(triggerReasonLabel(dto({ triggerReason: 'none' }))).toBe('未触发压缩')
  })

  test('formats counts and clamps the pressure meter', () => {
    expect(compactTokenCount(512)).toBe('512')
    expect(compactTokenCount(2048)).toBe('2.0K')
    expect(compactTokenCount(256_000)).toBe('256K')
    expect(compactTokenCount(4_000_000)).toBe('4.0M')
    expect(pressurePercent(dto({ pressureRatio: 1.4 }))).toBe(100)
    expect(pressurePercent(dto({ pressureRatio: -1 }))).toBe(0)
  })
})
