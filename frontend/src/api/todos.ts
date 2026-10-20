import { request } from './client'

export interface Todo {
  /** Equals createdAt. */
  id: string
  title: string
  project: string | null
  /** Calendar date, YYYY-MM-DD. */
  due: string | null
  done: boolean
  createdAt: string
  completedAt: string | null
  updatedAt: string
}

export interface NewTodo {
  title: string
  project?: string
  due?: string
}

/** Fields left out are unchanged; null clears project or due. */
export interface TodoPatch {
  title?: string
  project?: string | null
  due?: string | null
  done?: boolean
}

const todoUrl = (id: string) => `/todos/${encodeURIComponent(id)}`

export function listTodos(): Promise<Todo[]> {
  return request<Todo[]>('/todos')
}

export function createTodo(input: NewTodo): Promise<Todo> {
  return request<Todo>('/todos', { method: 'POST', body: JSON.stringify(input) })
}

export function updateTodo(id: string, patch: TodoPatch): Promise<Todo> {
  return request<Todo>(todoUrl(id), { method: 'PATCH', body: JSON.stringify(patch) })
}

export async function deleteTodo(id: string): Promise<void> {
  await request<void>(todoUrl(id), { method: 'DELETE' })
}
