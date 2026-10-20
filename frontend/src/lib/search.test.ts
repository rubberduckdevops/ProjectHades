import { describe, expect, it } from 'vitest'
import {
  folderBreadcrumb,
  formatScore,
  highlightParts,
  projectsOf,
  queryTerms,
  shouldAutoSearch,
} from './search'

const joined = (parts: { text: string }[]) => parts.map((p) => p.text).join('')

describe('queryTerms', () => {
  it('splits on whitespace, dedupes case-insensitively, longest first', () => {
    expect(queryTerms('  s3  Design design vectors ')).toEqual(['vectors', 'Design', 's3'])
    expect(queryTerms('   ')).toEqual([])
  })
})

describe('highlightParts', () => {
  it('marks case-insensitive matches and keeps the original text', () => {
    const parts = highlightParts('Design of the DESIGN doc', 'design')
    expect(parts).toEqual([
      { text: 'Design', match: true },
      { text: ' of the ', match: false },
      { text: 'DESIGN', match: true },
      { text: ' doc', match: false },
    ])
  })

  it('handles multiple terms and prefers the longest overlap', () => {
    const parts = highlightParts('storage and store', 'stor storage')
    expect(parts).toEqual([
      { text: 'storage', match: true },
      { text: ' and ', match: false },
      { text: 'stor', match: true },
      { text: 'e', match: false },
    ])
  })

  it('treats regex metacharacters literally', () => {
    const parts = highlightParts('a+b (c) [d] a.b', 'a+b (c) .')
    expect(parts.filter((p) => p.match).map((p) => p.text)).toEqual(['a+b', '(c)', '.'])
    expect(joined(parts)).toBe('a+b (c) [d] a.b')
  })

  it('returns the whole text unmarked for an empty query, and nothing for empty text', () => {
    expect(highlightParts('hello', '  ')).toEqual([{ text: 'hello', match: false }])
    expect(highlightParts('', 'x')).toEqual([])
  })

  it('never interprets HTML; it is just text', () => {
    const parts = highlightParts('<b>bold</b>', 'bold')
    expect(joined(parts)).toBe('<b>bold</b>')
    expect(parts.filter((p) => p.match)).toHaveLength(1)
  })

  it('works with non-ASCII text', () => {
    const parts = highlightParts('Überblick über alles', 'über')
    expect(parts.filter((p) => p.match).map((p) => p.text)).toEqual(['Über', 'über'])
  })
})

describe('shouldAutoSearch', () => {
  it('needs at least two non-space characters', () => {
    expect(shouldAutoSearch('')).toBe(false)
    expect(shouldAutoSearch(' a ')).toBe(false)
    expect(shouldAutoSearch('ab')).toBe(true)
  })
})

describe('formatScore', () => {
  it('formats as a clamped whole percentage', () => {
    expect(formatScore(0.834)).toBe('83%')
    expect(formatScore(1.2)).toBe('100%')
    expect(formatScore(-0.1)).toBe('0%')
    expect(formatScore(null)).toBeNull()
    expect(formatScore(Number.NaN)).toBeNull()
  })
})

describe('projectsOf', () => {
  it('returns distinct non-null projects sorted', () => {
    expect(projectsOf([{ project: 'hades' }, { project: null }, { project: 'Home' }, { project: 'hades' }])).toEqual([
      'hades',
      'Home',
    ])
  })
})

describe('folderBreadcrumb', () => {
  it('joins folder segments', () => {
    expect(folderBreadcrumb('Projects/Hades')).toBe('Projects › Hades')
    expect(folderBreadcrumb('')).toBe('')
  })
})
