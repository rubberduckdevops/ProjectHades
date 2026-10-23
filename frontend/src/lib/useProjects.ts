import { useCallback, useEffect, useState } from 'react'
import { listProjects } from '../api/projects'
import { loadCachedProjects, rememberProject, saveCachedProjects } from './projects'

/**
 * Project names for suggestions, most recently used first. Starts from the cached copy and
 * refreshes from the server on mount. `remember` moves a just-used name to the front; the
 * server records the use itself when the entry or todo is saved.
 */
export function useProjects(onError?: (err: unknown) => void) {
  const [projects, setProjects] = useState(loadCachedProjects)

  useEffect(() => {
    let cancelled = false
    listProjects()
      .then((list) => {
        if (cancelled) return
        const names = list.map((p) => p.name)
        saveCachedProjects(names)
        setProjects(names)
      })
      .catch((err: unknown) => {
        if (!cancelled) onError?.(err)
      })
    return () => {
      cancelled = true
    }
  }, [onError])

  const remember = useCallback((name: string) => {
    setProjects((prev) => {
      const next = rememberProject(prev, name)
      saveCachedProjects(next)
      return next
    })
  }, [])

  return { projects, remember }
}
