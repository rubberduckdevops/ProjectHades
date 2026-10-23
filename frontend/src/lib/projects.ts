// The server keeps the project list (GET /api/projects). This browser keeps a copy so
// suggestions are there before the request finishes, or if it fails.
const STORAGE_KEY = 'hades.recentProjects'

/** Most-recent-first, case-insensitively unique. */
export function mergeProjects(front: string[], rest: string[]): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const raw of [...front, ...rest]) {
    const name = raw.trim()
    const key = name.toLowerCase()
    if (!name || seen.has(key)) continue
    seen.add(key)
    out.push(name)
  }
  return out
}

/** `projects` with `name` moved to the front. */
export function rememberProject(projects: string[], name: string): string[] {
  return mergeProjects([name], projects)
}

/** The cached project list. Empty on any storage problem. */
export function loadCachedProjects(): string[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '[]')
    if (!Array.isArray(parsed)) return []
    return mergeProjects(
      parsed.filter((p): p is string => typeof p === 'string'),
      [],
    )
  } catch {
    return []
  }
}

export function saveCachedProjects(projects: string[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(projects))
  } catch {
    // Storage unavailable: the list just loads from the server each time.
  }
}
