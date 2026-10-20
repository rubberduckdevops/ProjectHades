import { fetchAuthSession } from 'aws-amplify/auth'
import { getConfig } from '../config'

export interface JournalEntry {
  id: string
  timestamp: string
  /** Free-text project; null for a general entry. */
  project: string | null
  body: string
}

async function getAccessToken(): Promise<string> {
  const session = await fetchAuthSession()
  const token = session.tokens?.accessToken?.toString()
  if (!token) throw new Error('Not signed in')
  return token
}

export interface AccessTokenInfo {
  token: string
  /** Expiry as epoch milliseconds. */
  expiresAt: number
}

/** The current access token and its expiry. `forceRefresh` mints a new one. */
export async function getAccessTokenInfo(forceRefresh = false): Promise<AccessTokenInfo> {
  const session = await fetchAuthSession({ forceRefresh })
  const accessToken = session.tokens?.accessToken
  if (!accessToken) throw new Error('Not signed in')
  const exp = accessToken.payload.exp
  if (typeof exp !== 'number') throw new Error('Access token has no expiry')
  return { token: accessToken.toString(), expiresAt: exp * 1000 }
}

/** An HTTP error from the API. `currentHash` is set on notes 409s when the note exists. */
export class ApiError extends Error {
  readonly status: number
  readonly currentHash?: string

  constructor(status: number, message: string, currentHash?: string) {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.currentHash = currentHash
  }
}

/**
 * Authenticated JSON request against the configured API base.
 * Resolves to `undefined` for 204 responses. Throws ApiError for non-2xx.
 */
export async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const token = await getAccessToken()
  const headers = new Headers(init.headers)
  headers.set('Authorization', `Bearer ${token}`)
  if (init.body !== undefined && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json')
  }

  const res = await fetch(`${getConfig().apiUrl}${path}`, { ...init, headers })
  if (!res.ok) {
    let message = `Request failed (HTTP ${res.status})`
    let currentHash: string | undefined
    try {
      const data = (await res.json()) as { error?: unknown; currentHash?: unknown }
      if (typeof data?.error === 'string' && data.error) message = data.error
      if (typeof data?.currentHash === 'string' && data.currentHash) currentHash = data.currentHash
    } catch {
      // non-JSON error body; keep default message
    }
    throw new ApiError(res.status, message, currentHash)
  }
  if (res.status === 204) return undefined as T
  return (await res.json()) as T
}

export function listJournal(from: string, to: string): Promise<JournalEntry[]> {
  return request<JournalEntry[]>(
    `/journal?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`,
  )
}

export function upsertJournal(input: { body: string; project?: string; id?: string }): Promise<JournalEntry> {
  return request<JournalEntry>('/journal', {
    method: 'PUT',
    body: JSON.stringify(input),
  })
}
