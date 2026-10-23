import { beforeEach, describe, expect, it, vi } from 'vitest'

const store = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => void store.set(k, v),
})

const { loadCachedProjects, mergeProjects, rememberProject, saveCachedProjects } = await import('./projects')

describe('projects', () => {
  beforeEach(() => store.clear())

  it('moves a used name to the front and dedupes case-insensitively', () => {
    expect(rememberProject(['Home', 'hades'], ' HADES ')).toEqual(['HADES', 'Home'])
  })

  it('ignores blank names', () => {
    expect(rememberProject(['a'], '   ')).toEqual(['a'])
    expect(mergeProjects(['', ' '], ['b'])).toEqual(['b'])
  })

  it('caches the list', () => {
    expect(loadCachedProjects()).toEqual([])
    saveCachedProjects(['hades', 'work'])
    expect(loadCachedProjects()).toEqual(['hades', 'work'])
  })

  it('survives junk in storage', () => {
    store.set('hades.recentProjects', '{bad')
    expect(loadCachedProjects()).toEqual([])
    store.set('hades.recentProjects', JSON.stringify(['ok', 3, null]))
    expect(loadCachedProjects()).toEqual(['ok'])
  })
})
