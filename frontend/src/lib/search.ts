/**
 * Pure helpers for the notes search UI (phase 2).
 */

/** Minimum trimmed query length before typing triggers a debounced search. */
export const MIN_AUTO_SEARCH_LENGTH = 2

/** Max query length the API accepts (after trimming). */
export const MAX_QUERY_LENGTH = 500

export interface HighlightPart {
  text: string
  match: boolean
}

const escapeRegExp = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

/** Distinct (case-insensitive) whitespace-separated terms, longest first. */
export function queryTerms(query: string): string[] {
  const seen = new Set<string>()
  const terms: string[] = []
  for (const term of query.trim().split(/\s+/)) {
    const key = term.toLowerCase()
    if (!term || seen.has(key)) continue
    seen.add(key)
    terms.push(term)
  }
  // Longest first so "design" wins over "des" in the alternation.
  return terms.sort((a, b) => b.length - a.length)
}

/**
 * Split `text` into plain and matching parts for every case-insensitive
 * occurrence of a query term. Joining the parts' text gives back `text`.
 * Render matching parts in <mark>; no HTML is ever produced here.
 */
export function highlightParts(text: string, query: string): HighlightPart[] {
  const terms = queryTerms(query)
  if (!text) return []
  if (terms.length === 0) return [{ text, match: false }]
  const re = new RegExp(`(${terms.map(escapeRegExp).join('|')})`, 'giu')
  const parts: HighlightPart[] = []
  // With one capture group, split() puts matches at odd indexes.
  text.split(re).forEach((piece, i) => {
    if (piece) parts.push({ text: piece, match: i % 2 === 1 })
  })
  return parts
}

/** Whether a typed query is long enough to auto-search. */
export function shouldAutoSearch(query: string): boolean {
  return query.trim().length >= MIN_AUTO_SEARCH_LENGTH
}

/** Cosine similarity as a whole percentage ("83%"), or null. Clamped to 0–100. */
export function formatScore(score: number | null | undefined): string | null {
  if (score === null || score === undefined || !Number.isFinite(score)) return null
  const pct = Math.round(Math.min(1, Math.max(0, score)) * 100)
  return `${pct}%`
}

/** Distinct non-null projects, sorted case-insensitively. */
export function projectsOf(notes: Iterable<{ project: string | null }>): string[] {
  const set = new Set<string>()
  for (const n of notes) if (n.project) set.add(n.project)
  return [...set].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: 'base' }))
}

/** Breadcrumb text for a folder ("Projects › Hades"), or "" for a top-level note. */
export function folderBreadcrumb(folder: string): string {
  return folder.split('/').filter(Boolean).join(' › ')
}
