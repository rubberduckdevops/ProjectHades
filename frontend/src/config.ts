import { Amplify } from 'aws-amplify'

export interface AppConfig {
  apiUrl: string
  region: string
  userPoolId: string
  userPoolClientId: string
}

const REQUIRED_KEYS = ['apiUrl', 'region', 'userPoolId', 'userPoolClientId'] as const

let config: AppConfig | null = null

export function getConfig(): AppConfig {
  if (!config) throw new Error('Config not loaded; call loadConfig() first')
  return config
}

export function parseConfig(raw: unknown): AppConfig {
  if (typeof raw !== 'object' || raw === null) {
    throw new Error('config.json must be a JSON object')
  }
  const obj = raw as Record<string, unknown>
  for (const key of REQUIRED_KEYS) {
    const value = obj[key]
    if (typeof value !== 'string' || value.trim() === '') {
      throw new Error(`config.json: "${key}" must be a non-empty string`)
    }
  }
  return {
    apiUrl: (obj.apiUrl as string).replace(/\/+$/, ''),
    region: obj.region as string,
    userPoolId: obj.userPoolId as string,
    userPoolClientId: obj.userPoolClientId as string,
  }
}

export async function loadConfig(): Promise<AppConfig> {
  const res = await fetch('/config.json', { cache: 'no-store' })
  if (!res.ok) {
    throw new Error(`Failed to fetch /config.json (HTTP ${res.status})`)
  }
  let raw: unknown
  try {
    raw = await res.json()
  } catch {
    throw new Error('/config.json is not valid JSON')
  }
  const parsed = parseConfig(raw)
  Amplify.configure({
    Auth: {
      Cognito: {
        userPoolId: parsed.userPoolId,
        userPoolClientId: parsed.userPoolClientId,
        loginWith: { email: true },
      },
    },
  })
  config = parsed
  return parsed
}
