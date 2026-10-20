import { describe, expect, it } from 'vitest'
import type { Todo } from '../api/todos'
import { dueStatus, formatDue, sortDone, sortOpen, upsertTodo } from './todos'

function todo(id: string, extra: Partial<Todo> = {}): Todo {
  return {
    id,
    title: id,
    project: null,
    due: null,
    done: false,
    createdAt: id,
    completedAt: null,
    updatedAt: id,
    ...extra,
  }
}

describe('sortOpen', () => {
  it('orders by due date, undated last, then by creation', () => {
    const items = [
      todo('2026-10-01T00:00:00.000Z'),
      todo('2026-10-02T00:00:00.000Z', { due: '2026-11-05' }),
      todo('2026-10-03T00:00:00.000Z', { due: '2026-10-25' }),
      todo('2026-09-30T00:00:00.000Z'),
      todo('2026-10-04T00:00:00.000Z', { due: '2026-10-25' }),
      todo('2026-10-05T00:00:00.000Z', { done: true }),
    ]
    expect(sortOpen(items).map((t) => t.id)).toEqual([
      '2026-10-03T00:00:00.000Z',
      '2026-10-04T00:00:00.000Z',
      '2026-10-02T00:00:00.000Z',
      '2026-09-30T00:00:00.000Z',
      '2026-10-01T00:00:00.000Z',
    ])
  })
})

describe('sortDone', () => {
  it('keeps only done todos, most recently completed first', () => {
    const items = [
      todo('a', { done: true, completedAt: '2026-10-01T00:00:00.000Z' }),
      todo('b'),
      todo('c', { done: true, completedAt: '2026-10-03T00:00:00.000Z' }),
    ]
    expect(sortDone(items).map((t) => t.id)).toEqual(['c', 'a'])
  })
})

describe('dueStatus', () => {
  it('compares calendar days', () => {
    expect(dueStatus(null, '2026-10-20')).toBe('none')
    expect(dueStatus('2026-10-19', '2026-10-20')).toBe('overdue')
    expect(dueStatus('2026-10-20', '2026-10-20')).toBe('today')
    expect(dueStatus('2026-10-21', '2026-10-20')).toBe('upcoming')
  })
})

describe('formatDue', () => {
  it('uses relative words near today', () => {
    expect(formatDue('2026-10-20', '2026-10-20')).toBe('Today')
    expect(formatDue('2026-10-21', '2026-10-20')).toBe('Tomorrow')
    expect(formatDue('2026-10-19', '2026-10-20')).toBe('Yesterday')
    expect(formatDue('2026-11-01', '2026-10-31')).toBe('Tomorrow')
  })

  it('shows the year only when it differs', () => {
    expect(formatDue('2026-11-02', '2026-10-20')).not.toMatch(/2026/)
    expect(formatDue('2027-01-15', '2026-10-20')).toMatch(/2027/)
  })
})

describe('upsertTodo', () => {
  it('replaces by id or appends', () => {
    const items = [todo('a'), todo('b')]
    const replaced = upsertTodo(items, todo('a', { title: 'new' }))
    expect(replaced.map((t) => t.title)).toEqual(['new', 'b'])
    expect(items[0].title).toBe('a')
    expect(upsertTodo(items, todo('c')).map((t) => t.id)).toEqual(['a', 'b', 'c'])
  })
})
