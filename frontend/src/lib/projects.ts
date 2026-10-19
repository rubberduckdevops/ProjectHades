const STORAGE_KEY = 'hades.recentProjects'
export const MAX_RECENT_PROJECTS = 20

/** Most-recent-first, case-insensitively unique, capped. */
export function mergeProjects(front: string[], rest: string[]): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const raw of [...front, ...rest]) {
    const name = raw.trim()
    const key = name.toLowerCase()
    if (!name || seen.has(key)) continue
    seen.add(key)
    out.push(name)
    if (out.length === MAX_RECENT_PROJECTS) break
  }
  return out
}

/** Recently used project names, most recent first. Empty on any storage problem. */
export function loadRecentProjects(): string[] {
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

function save(projects: string[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(projects))
  } catch {
    // Storage unavailable: suggestions just won't persist.
  }
}

/** Move `name` to the front of the recent list. Returns the new list. */
export function rememberProject(name: string): string[] {
  const next = mergeProjects([name], loadRecentProjects())
  save(next)
  return next
}

/** Add names seen elsewhere (e.g. loaded entries) after the existing ones. Returns the new list. */
export function absorbProjects(names: string[]): string[] {
  const current = loadRecentProjects()
  const next = mergeProjects(current, names)
  if (next.length !== current.length) save(next)
  return next
}
