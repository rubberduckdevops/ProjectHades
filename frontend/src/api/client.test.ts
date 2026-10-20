import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('aws-amplify/auth', () => ({
  fetchAuthSession: vi.fn(async () => ({
    tokens: { accessToken: { toString: () => 'test-token', payload: { exp: 2_000_000_000 } } },
  })),
}))

vi.mock('../config', () => ({
  getConfig: () => ({ apiUrl: 'https://api.test/api' }),
}))

import { ApiError, request } from './client'
import { createNote, deleteNote, getNote, listNotes, moveNote, searchNotes, searchUrl, updateNote } from './notes'
import { createTodo, deleteTodo, listTodos, updateTodo } from './todos'

const fetchMock = vi.fn<typeof fetch>()

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

beforeEach(() => {
  fetchMock.mockReset()
  vi.stubGlobal('fetch', fetchMock)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

function lastCall(): { url: string; init: RequestInit; headers: Headers } {
  const [url, init = {}] = fetchMock.mock.calls.at(-1)!
  return { url: String(url), init, headers: new Headers(init.headers) }
}

describe('request', () => {
  it('sends the bearer token and parses JSON', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse(200, { ok: true }))
    await expect(request('/thing')).resolves.toEqual({ ok: true })
    const { url, headers } = lastCall()
    expect(url).toBe('https://api.test/api/thing')
    expect(headers.get('Authorization')).toBe('Bearer test-token')
    expect(headers.has('Content-Type')).toBe(false)
  })

  it('sets JSON content type when there is a body', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse(200, {}))
    await request('/thing', { method: 'PUT', body: '{}' })
    expect(lastCall().headers.get('Content-Type')).toBe('application/json')
  })

  it('resolves undefined on 204 without reading a body', async () => {
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }))
    await expect(request('/thing', { method: 'DELETE' })).resolves.toBeUndefined()
  })

  it('throws ApiError with status, message and currentHash on 409', async () => {
    const hash = 'a'.repeat(64)
    fetchMock.mockResolvedValueOnce(jsonResponse(409, { error: 'note was changed', currentHash: hash }))
    const err = await request('/notes/x.md').catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ApiError)
    expect(err).toBeInstanceOf(Error)
    expect(err).toMatchObject({ status: 409, message: 'note was changed', currentHash: hash })
  })

  it('leaves currentHash undefined when absent', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse(409, { error: 'target exists' }))
    const err = (await request('/notes/move').catch((e: unknown) => e)) as ApiError
    expect(err.status).toBe(409)
    expect(err.message).toBe('target exists')
    expect(err.currentHash).toBeUndefined()
  })

  it('falls back to a generic message for non-JSON error bodies', async () => {
    fetchMock.mockResolvedValueOnce(new Response('Bad Gateway', { status: 502 }))
    const err = (await request('/x').catch((e: unknown) => e)) as ApiError
    expect(err).toBeInstanceOf(ApiError)
    expect(err.status).toBe(502)
    expect(err.message).toBe('Request failed (HTTP 502)')
    expect(err.currentHash).toBeUndefined()
  })
})

describe('notes API', () => {
  const hash = 'b'.repeat(64)

  it('lists notes with an optional encoded prefix', async () => {
    fetchMock.mockImplementation(async () => jsonResponse(200, []))
    await listNotes()
    expect(lastCall().url).toBe('https://api.test/api/notes')
    await listNotes('My Folder/')
    expect(lastCall().url).toBe('https://api.test/api/notes?prefix=My%20Folder%2F')
  })

  it('gets a note with per-segment encoding', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse(200, {}))
    await getNote('Projects/My Ideas/Design #1.md')
    expect(lastCall().url).toBe('https://api.test/api/notes/Projects/My%20Ideas/Design%20%231.md')
    expect(lastCall().init.method).toBeUndefined()
  })

  it('creates without baseHash and updates with it', async () => {
    fetchMock.mockImplementation(async () => jsonResponse(200, {}))
    await createNote('a/b.md', '# Hi')
    expect(lastCall().init.method).toBe('PUT')
    expect(JSON.parse(lastCall().init.body as string)).toEqual({ content: '# Hi' })
    await updateNote('a/b.md', '', hash)
    expect(lastCall().url).toBe('https://api.test/api/notes/a/b.md')
    expect(JSON.parse(lastCall().init.body as string)).toEqual({ content: '', baseHash: hash })
  })

  it('deletes with baseHash in the query string', async () => {
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }))
    await expect(deleteNote('a b/c.md', hash)).resolves.toBeUndefined()
    expect(lastCall().url).toBe(`https://api.test/api/notes/a%20b/c.md?baseHash=${hash}`)
    expect(lastCall().init.method).toBe('DELETE')
  })

  it('moves via POST /notes/move', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse(200, {}))
    await moveNote('a.md', 'b/a.md', hash)
    expect(lastCall().url).toBe('https://api.test/api/notes/move')
    expect(lastCall().init.method).toBe('POST')
    expect(JSON.parse(lastCall().init.body as string)).toEqual({ from: 'a.md', to: 'b/a.md', baseHash: hash })
  })

  it('searches with each parameter encoded', async () => {
    fetchMock.mockImplementation(async () => jsonResponse(200, []))
    await searchNotes('s3 & vectors?')
    expect(lastCall().url).toBe('https://api.test/api/search?q=s3%20%26%20vectors%3F')
    expect(lastCall().init.method).toBeUndefined()
    await searchNotes('design #1', { project: 'Home Lab', limit: 20 })
    expect(lastCall().url).toBe('https://api.test/api/search?q=design%20%231&project=Home%20Lab&limit=20')
  })

  it('passes an abort signal through', async () => {
    fetchMock.mockImplementation(async () => jsonResponse(200, []))
    const controller = new AbortController()
    await searchNotes('x', {}, controller.signal)
    expect(lastCall().init.signal).toBe(controller.signal)
  })
})

describe('searchUrl', () => {
  it('omits empty project and absent limit', () => {
    expect(searchUrl('a')).toBe('/search?q=a')
    expect(searchUrl('a', { project: '' })).toBe('/search?q=a')
    expect(searchUrl('a', { project: null, limit: 5 })).toBe('/search?q=a&limit=5')
    expect(searchUrl('a/b', { project: 'x/y' })).toBe('/search?q=a%2Fb&project=x%2Fy')
  })
})

describe('todos API', () => {
  const id = '2026-10-20T14:03:22.123Z'

  it('lists and creates', async () => {
    fetchMock.mockImplementation(async () => jsonResponse(200, []))
    await listTodos()
    expect(lastCall().url).toBe('https://api.test/api/todos')
    await createTodo({ title: 'Renew passport', due: '2026-11-01' })
    expect(lastCall().init.method).toBe('POST')
    expect(JSON.parse(lastCall().init.body as string)).toEqual({ title: 'Renew passport', due: '2026-11-01' })
  })

  it('patches by encoded id, keeping nulls', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse(200, {}))
    await updateTodo(id, { done: true, due: null })
    expect(lastCall().url).toBe('https://api.test/api/todos/2026-10-20T14%3A03%3A22.123Z')
    expect(lastCall().init.method).toBe('PATCH')
    expect(JSON.parse(lastCall().init.body as string)).toEqual({ done: true, due: null })
  })

  it('deletes by id', async () => {
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }))
    await expect(deleteTodo(id)).resolves.toBeUndefined()
    expect(lastCall().init.method).toBe('DELETE')
  })
})
