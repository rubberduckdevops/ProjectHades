/**
 * Note path helpers. Rules mirror API_CONTRACT.md "Notes (phase 1) → Paths".
 */

export const MAX_NOTE_PATH_BYTES = 512

// U+0000–U+001F and U+007F.
// oxlint-disable-next-line no-control-regex
const CONTROL_CHARS = /[\u0000-\u001f\u007f]/
// oxlint-disable-next-line no-control-regex
const CONTROL_CHARS_GLOBAL = /[\u0000-\u001f\u007f]/g
// Unicode White_Space (matches Rust's char::is_whitespace).
const LEADING_OR_TRAILING_WS = /^\p{White_Space}|\p{White_Space}$/u
const WS_EDGES = /^\p{White_Space}+|\p{White_Space}+$/gu

const encoder = new TextEncoder()

export function utf8Length(s: string): number {
  return encoder.encode(s).length
}

/** Returns a human-readable error for an invalid note path, or null when valid. */
export function validateNotePath(path: string): string | null {
  if (path.length === 0) return 'Path is required'
  if (CONTROL_CHARS.test(path)) return 'Path must not contain control characters'
  if (path.includes('\\')) return 'Path must not contain a backslash'
  if (path.startsWith('/')) return 'Path must not start with "/"'
  if (path.endsWith('/')) return 'Path must not end with "/"'
  if (!path.endsWith('.md')) return 'Path must end in ".md"'
  const bytes = utf8Length(path)
  if (bytes > MAX_NOTE_PATH_BYTES) {
    return `Path is too long (${bytes} bytes; max ${MAX_NOTE_PATH_BYTES})`
  }
  for (const segment of path.split('/')) {
    if (segment === '') return 'Path must not contain empty segments ("//")'
    if (segment === '.' || segment === '..') return 'Path segments must not be "." or ".."'
    if (LEADING_OR_TRAILING_WS.test(segment)) {
      return 'Path segments must not start or end with whitespace'
    }
  }
  return null
}

/** Percent-encode each segment on its own, joined with "/". */
export function encodeNotePath(path: string): string {
  return path.split('/').map(encodeURIComponent).join('/')
}

/** Inverse of encodeNotePath. Throws URIError on malformed escapes. */
export function decodeNotePath(encoded: string): string {
  return encoded.split('/').map(decodeURIComponent).join('/')
}

function cleanSegment(segment: string): string {
  return segment.replace(CONTROL_CHARS_GLOBAL, '').replace(WS_EDGES, '')
}

function cleanFolder(folder: string): string[] {
  return folder
    .replace(/\\/g, '/')
    .split('/')
    .map(cleanSegment)
    .filter((s) => s !== '' && s !== '.' && s !== '..')
}

/**
 * Default path for a new note: `<folder>/<title>.md`, sanitized so the result
 * is valid whenever possible. Folder backslashes become "/", slashes collapse,
 * segments are trimmed and empty/"."/".." segments dropped. In the title, "/"
 * and "\" become "-" (a title never creates folders). An empty title gives
 * "Untitled.md"; a title already ending in ".md" is not suffixed twice.
 */
export function defaultPathFor(folder: string, title: string): string {
  let name = cleanSegment(title.replace(/[/\\]/g, '-'))
  if (name.toLowerCase().endsWith('.md')) name = name.slice(0, -3).replace(WS_EDGES, '')
  if (name === '' || name === '.' || name === '..') name = 'Untitled'
  return [...cleanFolder(folder), `${name}.md`].join('/')
}

/** Folder part of a path ("" for a top-level note). */
export function folderOf(path: string): string {
  const i = path.lastIndexOf('/')
  return i === -1 ? '' : path.slice(0, i)
}

/** File name part of a path (including ".md"). */
export function fileNameOf(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1)
}

/** Distinct, sorted folders (and their ancestors) for the given note paths. */
export function foldersOf(paths: Iterable<string>): string[] {
  const set = new Set<string>()
  for (const p of paths) {
    const parts = folderOf(p).split('/').filter(Boolean)
    for (let i = 1; i <= parts.length; i++) set.add(parts.slice(0, i).join('/'))
  }
  return [...set].sort((a, b) => a.localeCompare(b))
}
