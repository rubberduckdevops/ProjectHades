import { beforeEach, describe, expect, it, vi } from 'vitest'

const store = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => void store.set(k, v),
})

const { MAX_RECENT_PROJECTS, absorbProjects, loadRecentProjects, rememberProject } = await import('./projects')

describe('recent projects', () => {
  beforeEach(() => store.clear())

  it('starts empty', () => {
    expect(loadRecentProjects()).toEqual([])
  })

  it('remembers most recent first and dedupes case-insensitively', () => {
    rememberProject('hades')
    rememberProject('Home')
    expect(rememberProject(' HADES ')).toEqual(['HADES', 'Home'])
  })

  it('ignores blank names', () => {
    expect(rememberProject('   ')).toEqual([])
  })

  it('caps the list', () => {
    for (let i = 0; i < MAX_RECENT_PROJECTS + 5; i++) rememberProject(`p${i}`)
    const list = loadRecentProjects()
    expect(list).toHaveLength(MAX_RECENT_PROJECTS)
    expect(list[0]).toBe(`p${MAX_RECENT_PROJECTS + 4}`)
  })

  it('absorbs names after existing ones', () => {
    rememberProject('hades')
    expect(absorbProjects(['work', 'Hades'])).toEqual(['hades', 'work'])
  })

  it('survives junk in storage', () => {
    store.set('hades.recentProjects', '{bad')
    expect(loadRecentProjects()).toEqual([])
    store.set('hades.recentProjects', JSON.stringify(['ok', 3, null]))
    expect(loadRecentProjects()).toEqual(['ok'])
  })
})
