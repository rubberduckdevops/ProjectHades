import { describe, expect, it } from 'vitest'
import { activeApplicationHref } from './applications'

describe('activeApplicationHref', () => {
  it.each([
    ['/notes', '/notes'],
    ['/notes/new', '/notes'],
    ['/notes/Projects/Design.md', '/notes'],
    ['/journal', '/journal'],
    ['/notesy', '/notesy'],
    ['/settings', '/settings'],
  ])('%s -> %s', (pathname, expected) => {
    expect(activeApplicationHref(pathname)).toBe(expected)
  })
})
