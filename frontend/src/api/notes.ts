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
  /** True when the search index reflects the current content (phase 2). */
  indexed: boolean
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

/** One hit from GET /search. Name matches rank first; see API_CONTRACT.md "Notes search". */
export interface SearchResult {
  path: string
  title: string
  project: string | null
  updatedAt: string
  match: 'name' | 'semantic'
  /** Cosine similarity (1 - distance); null for a name match with no semantic hit. */
  score: number | null
  heading: string | null
  snippet: string | null
}

export interface SearchOptions {
  project?: string | null
  limit?: number
}

/** Relative URL for GET /search; empty/absent options are omitted. */
export function searchUrl(q: string, { project, limit }: SearchOptions = {}): string {
  let url = `/search?q=${encodeURIComponent(q)}`
  if (project) url += `&project=${encodeURIComponent(project)}`
  if (limit !== undefined) url += `&limit=${encodeURIComponent(String(limit))}`
  return url
}

/** GET /search?q=&project=&limit= */
export function searchNotes(q: string, options: SearchOptions = {}, signal?: AbortSignal): Promise<SearchResult[]> {
  return request<SearchResult[]>(searchUrl(q, options), { signal })
}
