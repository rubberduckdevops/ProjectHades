import type { Todo } from '../api/todos'

/** Open todos: dated ones first (soonest due first), then undated; ties oldest first. */
export function sortOpen(todos: Todo[]): Todo[] {
  return todos
    .filter((t) => !t.done)
    .sort((a, b) => {
      if (a.due !== b.due) {
        if (a.due === null) return 1
        if (b.due === null) return -1
        return a.due.localeCompare(b.due)
      }
      return a.createdAt.localeCompare(b.createdAt)
    })
}

/** Done todos, most recently completed first. */
export function sortDone(todos: Todo[]): Todo[] {
  return todos
    .filter((t) => t.done)
    .sort((a, b) => (b.completedAt ?? b.updatedAt).localeCompare(a.completedAt ?? a.updatedAt))
}

export type DueStatus = 'overdue' | 'today' | 'upcoming' | 'none'

/** How `due` relates to `today` (both YYYY-MM-DD, compared as calendar days). */
export function dueStatus(due: string | null, today: string): DueStatus {
  if (!due) return 'none'
  if (due < today) return 'overdue'
  if (due === today) return 'today'
  return 'upcoming'
}

/** Whole days from `from` to `to` (YYYY-MM-DD). */
function daysBetween(from: string, to: string): number {
  return Math.round((Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)) / 86_400_000)
}

/** "Today", "Tomorrow", "Yesterday", else e.g. "Mon, Nov 2" (with the year if it differs). */
export function formatDue(due: string, today: string): string {
  const diff = daysBetween(today, due)
  if (diff === 0) return 'Today'
  if (diff === 1) return 'Tomorrow'
  if (diff === -1) return 'Yesterday'
  const date = new Date(`${due}T00:00:00Z`)
  return date.toLocaleDateString(undefined, {
    timeZone: 'UTC',
    weekday: 'short',
    month: 'short',
    day: 'numeric',
    ...(due.slice(0, 4) !== today.slice(0, 4) ? { year: 'numeric' } : {}),
  })
}

/** Replace (or add) `todo` by id. */
export function upsertTodo(todos: Todo[], todo: Todo): Todo[] {
  const i = todos.findIndex((t) => t.id === todo.id)
  if (i === -1) return [...todos, todo]
  const next = [...todos]
  next[i] = todo
  return next
}
