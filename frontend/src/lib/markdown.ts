import type { JournalEntry } from '../api/client'
import { formatLongDate, formatTime } from './dates'

/** Make text safe inside a Markdown table cell (Obsidian/GFM). */
export function escapeTableCell(text: string): string {
  return text
    .replace(/\\/g, '\\\\')
    .replace(/\|/g, '\\|')
    .replace(/\r\n?/g, '\n')
    .trim()
    .split('\n')
    .join('<br>')
}

/** Render a day's entries as a Markdown heading plus a Time | Entry table. */
export function journalToMarkdown(entries: JournalEntry[], day: Date, timeZone: string): string {
  const lines = [
    `## Journal — ${formatLongDate(day, timeZone)}`,
    '',
    '| Time | Entry |',
    '| --- | --- |',
    ...entries.map((e) => `| ${formatTime(e.timestamp, timeZone)} | ${escapeTableCell(e.body)} |`),
  ]
  return lines.join('\n') + '\n'
}
