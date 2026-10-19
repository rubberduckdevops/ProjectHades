import { describe, expect, it } from 'vitest'
import { escapeTableCell, journalToMarkdown } from './markdown'

describe('escapeTableCell', () => {
  it('escapes pipes and backslashes', () => {
    expect(escapeTableCell('a | b \\ c')).toBe('a \\| b \\\\ c')
  })

  it('turns newlines into <br> so multi-line entries stay in one row', () => {
    expect(escapeTableCell('line one\r\nline two\nline three\n')).toBe('line one<br>line two<br>line three')
  })
})

describe('journalToMarkdown', () => {
  it('renders a Time | Project | Entry table in the given timezone, blank project for general entries', () => {
    const md = journalToMarkdown(
      [
        { id: 'a', timestamp: '2026-10-19T16:14:00.000Z', project: 'hades', body: 'Started the backend' },
        { id: 'b', timestamp: '2026-10-19T19:02:30.000Z', project: null, body: 'Lunch | then review\nsecond line' },
      ],
      new Date('2026-10-19T20:00:00Z'),
      'America/Los_Angeles',
    )
    expect(md).toBe(
      [
        '## Journal — Monday, October 19, 2026',
        '',
        '| Time | Project | Entry |',
        '| --- | --- | --- |',
        '| 09:14:00 | hades | Started the backend |',
        '| 12:02:30 |  | Lunch \\| then review<br>second line |',
        '',
      ].join('\n'),
    )
  })
})
