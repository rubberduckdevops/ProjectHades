import { describe, expect, it } from 'vitest'
import {
  decodeNotePath,
  defaultPathFor,
  encodeNotePath,
  fileNameOf,
  folderOf,
  foldersOf,
  utf8Length,
  validateNotePath,
} from './notePaths'

describe('validateNotePath', () => {
  it.each([
    'Design.md',
    'Projects/Hades/Design.md',
    'Projects/My Ideas/Design.md',
    'a/b c/d.md',
    'Ünïcödé/日本語.md',
    'emoji/🚀.md',
    'with.dots/file.name.md',
    '...md',
    'a/.hidden.md',
    'a/b%20c/?#&+.md',
    'Case/Sensitive.MD.md',
  ])('accepts %j', (path) => {
    expect(validateNotePath(path)).toBeNull()
  })

  it.each([
    ['', 'required'],
    ['Design', '.md'],
    ['Design.MD', '.md'],
    ['Design.md.txt', '.md'],
    ['/Design.md', 'start'],
    ['Projects/', 'end with'],
    ['Projects/Design.md/', 'end with'],
    ['a//b.md', 'empty'],
    ['./a.md', '"."'],
    ['a/../b.md', '".."'],
    ['a/./b.md', '"."'],
    ['a\\b.md', 'backslash'],
    ['a\u0000b.md', 'control'],
    ['a\nb.md', 'control'],
    ['a\tb.md', 'control'],
    ['a\u001fb.md', 'control'],
    ['a\u007fb.md', 'control'],
    [' a/b.md', 'whitespace'],
    ['a /b.md', 'whitespace'],
    ['a/ b.md', 'whitespace'],
    ['a/b.md ', '.md'],
    ['a /b.md', 'whitespace'],
    ['　a/b.md', 'whitespace'],
  ])('rejects %j', (path, fragment) => {
    const err = validateNotePath(path)
    expect(err).not.toBeNull()
    expect(err).toContain(fragment)
  })

  it('allows exactly 512 bytes and rejects 513 (ASCII)', () => {
    const ok = 'a'.repeat(509) + '.md'
    expect(ok.length).toBe(512)
    expect(validateNotePath(ok)).toBeNull()
    expect(validateNotePath('a' + ok)).toContain('too long')
  })

  it('counts UTF-8 bytes, not UTF-16 code units', () => {
    // "é" is 2 bytes in UTF-8; 254 of them + ".md" = 511 bytes, 257 chars.
    const ok = 'é'.repeat(254) + '.md'
    expect(utf8Length(ok)).toBe(511)
    expect(validateNotePath(ok)).toBeNull()
    const tooLong = 'é'.repeat(255) + '.md' // 513 bytes, only 258 chars
    expect(tooLong.length).toBeLessThan(512)
    expect(validateNotePath(tooLong)).toContain('too long')
    // "🚀" is 4 bytes (2 UTF-16 units): 127*4 + 3 = 511, 128*4 + 3 = 515.
    expect(validateNotePath('🚀'.repeat(127) + '.md')).toBeNull()
    expect(validateNotePath('🚀'.repeat(128) + '.md')).toContain('too long')
  })
})

describe('encodeNotePath / decodeNotePath', () => {
  it('encodes each segment separately, keeping "/" separators', () => {
    expect(encodeNotePath('Projects/My Ideas/Design.md')).toBe('Projects/My%20Ideas/Design.md')
    expect(encodeNotePath('a?b/c#d/e%f&g+h.md')).toBe('a%3Fb/c%23d/e%25f%26g%2Bh.md')
    expect(encodeNotePath('日本/ü.md')).toBe('%E6%97%A5%E6%9C%AC/%C3%BC.md')
  })

  it.each([
    'Design.md',
    'Projects/My Ideas/Design.md',
    'a?b/c#d/e%f&g+h.md',
    '日本語/🚀 rocket.md',
    "quotes/'single' \"double\".md",
    'percent/100%25.md',
  ])('round-trips %j', (path) => {
    expect(decodeNotePath(encodeNotePath(path))).toBe(path)
  })

  it('throws on malformed escapes', () => {
    expect(() => decodeNotePath('bad%zz.md')).toThrow(URIError)
  })
})

describe('defaultPathFor', () => {
  it.each([
    ['', 'Design', 'Design.md'],
    ['Projects/Hades', 'Design', 'Projects/Hades/Design.md'],
    ['/Projects//Hades/', 'Design', 'Projects/Hades/Design.md'],
    ['  Projects / Hades  ', '  Design  ', 'Projects/Hades/Design.md'],
    ['Projects\\Hades', 'Design', 'Projects/Hades/Design.md'],
    ['./a/../b', 'X', 'a/b/X.md'],
    ['', '', 'Untitled.md'],
    ['Inbox', '   ', 'Inbox/Untitled.md'],
    ['', 'A/B\\C', 'A-B-C.md'],
    ['', 'notes.md', 'notes.md'],
    ['', 'Tab\there', 'Tabhere.md'],
    ['f\u0000o', 'x\u007f', 'fo/x.md'],
    ['', '..', 'Untitled.md'],
    ['Ideas', 'Café ☕', 'Ideas/Café ☕.md'],
  ])('(%j, %j) -> %j', (folder, title, expected) => {
    expect(defaultPathFor(folder, title)).toBe(expected)
    expect(validateNotePath(expected)).toBeNull()
  })
})

describe('folderOf / fileNameOf / foldersOf', () => {
  it('splits paths', () => {
    expect(folderOf('Projects/Hades/Design.md')).toBe('Projects/Hades')
    expect(fileNameOf('Projects/Hades/Design.md')).toBe('Design.md')
    expect(folderOf('Design.md')).toBe('')
    expect(fileNameOf('Design.md')).toBe('Design.md')
  })

  it('collects folders with ancestors, sorted and distinct', () => {
    expect(foldersOf(['a.md', 'P/H/x.md', 'P/y.md', 'Z/z.md'])).toEqual(['P', 'P/H', 'Z'])
  })
})
