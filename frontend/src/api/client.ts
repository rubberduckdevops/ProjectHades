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

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const token = await getAccessToken()
  const headers = new Headers(init.headers)
  headers.set('Authorization', `Bearer ${token}`)
  headers.set('Content-Type', 'application/json')

  const res = await fetch(`${getConfig().apiUrl}${path}`, { ...init, headers })
  if (!res.ok) {
    let message = `Request failed (HTTP ${res.status})`
    try {
      const data = (await res.json()) as { error?: unknown }
      if (typeof data?.error === 'string' && data.error) message = data.error
    } catch {
      // non-JSON error body; keep default message
    }
    throw new Error(message)
  }
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
