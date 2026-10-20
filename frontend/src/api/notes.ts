import { encodeNotePath } from '../lib/notePaths'
import { request } from './client'

export interface NoteMeta {
  path: string
  title: string
  project: string | null
  tags: string[]
  /** Lowercase hex sha256 of the UTF-8 content. */
  contentHash: string
  size: number
  createdAt: string
  updatedAt: string
}

export interface Note extends NoteMeta {
  content: string
}

const notePath = (path: string) => `/notes/${encodeNotePath(path)}`

/** GET /notes[?prefix=] — sorted by path ascending. */
export function listNotes(prefix?: string): Promise<NoteMeta[]> {
  const query = prefix ? `?prefix=${encodeURIComponent(prefix)}` : ''
  return request<NoteMeta[]>(`/notes${query}`)
}

export function getNote(path: string): Promise<Note> {
  return request<Note>(notePath(path))
}

/** Create-only PUT (no baseHash). 409 if the note already exists. */
export function createNote(path: string, content: string): Promise<NoteMeta> {
  return request<NoteMeta>(notePath(path), {
    method: 'PUT',
    body: JSON.stringify({ content }),
  })
}

/** Update-only PUT. 409 (with currentHash) if the stored hash differs from baseHash. */
export function updateNote(path: string, content: string, baseHash: string): Promise<NoteMeta> {
  return request<NoteMeta>(notePath(path), {
    method: 'PUT',
    body: JSON.stringify({ content, baseHash }),
  })
}

export async function deleteNote(path: string, baseHash: string): Promise<void> {
  await request<void>(`${notePath(path)}?baseHash=${encodeURIComponent(baseHash)}`, {
    method: 'DELETE',
  })
}

export function moveNote(from: string, to: string, baseHash: string): Promise<NoteMeta> {
  return request<NoteMeta>('/notes/move', {
    method: 'POST',
    body: JSON.stringify({ from, to, baseHash }),
  })
}
