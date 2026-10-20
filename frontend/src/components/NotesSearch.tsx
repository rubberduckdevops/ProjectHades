import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import Alert from '@cloudscape-design/components/alert'
import Badge from '@cloudscape-design/components/badge'
import Box from '@cloudscape-design/components/box'
import Button from '@cloudscape-design/components/button'
import Container from '@cloudscape-design/components/container'
import Header from '@cloudscape-design/components/header'
import Input from '@cloudscape-design/components/input'
import Link from '@cloudscape-design/components/link'
import Select, { type SelectProps } from '@cloudscape-design/components/select'
import SpaceBetween from '@cloudscape-design/components/space-between'
import Spinner from '@cloudscape-design/components/spinner'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { searchNotes, type SearchResult } from '../api/notes'
import { encodeNotePath, folderOf } from '../lib/notePaths'
import {
  folderBreadcrumb,
  formatScore,
  highlightParts,
  MAX_QUERY_LENGTH,
  shouldAutoSearch,
} from '../lib/search'

const DEBOUNCE_MS = 400
const RESULT_LIMIT = 20
const ALL_PROJECTS: SelectProps.Option = { label: 'All projects', value: '' }

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

/** Text with query terms wrapped in <mark>. Plain React text nodes only. */
function Highlighted({ text, query }: { text: string; query: string }) {
  return (
    <>
      {highlightParts(text, query).map((part, i) =>
        part.match ? <mark key={i}>{part.text}</mark> : <span key={i}>{part.text}</span>,
      )}
    </>
  )
}

function ResultItem({ result, query }: { result: SearchResult; query: string }) {
  const navigate = useNavigate()
  const href = `/notes/${encodeNotePath(result.path)}`
  const folder = folderBreadcrumb(folderOf(result.path))
  const score = result.match === 'semantic' ? formatScore(result.score) : null
  return (
    <li style={{ listStyle: 'none', padding: '8px 0' }}>
      <SpaceBetween size="xxs">
        <SpaceBetween direction="horizontal" size="xs" alignItems="center">
          <Link
            fontSize="heading-s"
            href={href}
            onFollow={(e) => {
              e.preventDefault()
              navigate(href)
            }}
          >
            <Highlighted text={result.title} query={query} />
          </Link>
          <Badge color={result.match === 'name' ? 'blue' : 'grey'}>
            {result.match === 'name' ? 'Name match' : 'Semantic'}
          </Badge>
          {score && (
            <Box variant="small" color="text-status-inactive">
              <span title="Semantic similarity">{score}</span>
            </Box>
          )}
        </SpaceBetween>
        <Box variant="small" color="text-body-secondary">
          {[folder || '/', result.project].filter(Boolean).join(' · ')}
        </Box>
        {result.heading && (
          <Box variant="small" color="text-body-secondary" fontWeight="bold">
            {result.heading}
          </Box>
        )}
        {result.snippet ? (
          <Box variant="p" margin={{ top: 'xxs' }}>
            <Highlighted text={result.snippet} query={query} />
          </Box>
        ) : (
          <Box variant="code" color="text-body-secondary">
            <Highlighted text={result.path} query={query} />
          </Box>
        )}
      </SpaceBetween>
    </li>
  )
}

interface SearchState {
  results: SearchResult[]
  loading: boolean
  error: string | null
}

const IDLE: SearchState = { results: [], loading: false, error: null }

/**
 * Search box + results above the notes table. The submitted query lives in the
 * URL (`?q=`, plus `?project=`) so back/forward and shared links work; other
 * params (e.g. `?filter=`) are preserved.
 */
export default function NotesSearch({ projects }: { projects: string[] }) {
  const [searchParams, setSearchParams] = useSearchParams()
  const urlQuery = searchParams.get('q') ?? ''
  const urlProject = searchParams.get('project') ?? ''
  const activeQuery = urlQuery.trim()

  const [input, setInput] = useState(urlQuery)
  // Follow the URL on back/forward (adjusting state during render, not in an effect).
  const [seenUrlQuery, setSeenUrlQuery] = useState(urlQuery)
  if (seenUrlQuery !== urlQuery) {
    setSeenUrlQuery(urlQuery)
    if (urlQuery.trim() !== input.trim()) setInput(urlQuery)
  }

  const [state, setState] = useState<SearchState>(IDLE)
  // Bumped by the Search button / Enter so re-submitting the same query reruns it.
  const [runId, setRunId] = useState(0)

  const updateParams = useCallback(
    (changes: Record<string, string>, replace: boolean) => {
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev)
          for (const [key, value] of Object.entries(changes)) {
            if (value) next.set(key, value)
            else next.delete(key)
          }
          return next
        },
        { replace },
      )
    },
    [setSearchParams],
  )

  // Latest values for the debounce timer without re-arming it on every URL change.
  const latest = useRef({ activeQuery, updateParams })
  useEffect(() => {
    latest.current = { activeQuery, updateParams }
  })

  const submit = () => {
    const q = input.trim().slice(0, MAX_QUERY_LENGTH)
    if (q !== activeQuery) updateParams({ q }, false)
    else setRunId((n) => n + 1)
  }

  // Debounced search while typing; clearing the box clears results immediately.
  useEffect(() => {
    const q = input.trim().slice(0, MAX_QUERY_LENGTH)
    if (q === latest.current.activeQuery) return
    if (q === '') {
      latest.current.updateParams({ q: '' }, true)
      return
    }
    if (!shouldAutoSearch(q)) return
    const timer = setTimeout(() => {
      // Typing refines the current history entry; the first query of a session pushes one.
      latest.current.updateParams({ q }, latest.current.activeQuery !== '')
    }, DEBOUNCE_MS)
    return () => clearTimeout(timer)
  }, [input])

  // Run the search whenever the submitted query/project changes. Aborting the
  // previous request (and checking the signal) drops out-of-order responses.
  useEffect(() => {
    if (!activeQuery) return
    const controller = new AbortController()
    searchNotes(activeQuery, { project: urlProject || null, limit: RESULT_LIMIT }, controller.signal)
      .then((results) => {
        if (!controller.signal.aborted) setState({ results, loading: false, error: null })
      })
      .catch((err: unknown) => {
        if (!controller.signal.aborted) setState({ results: [], loading: false, error: errorMessage(err) })
      })
    return () => controller.abort()
  }, [activeQuery, urlProject, runId])

  // Mark loading as part of the render that changes the request key.
  const requestKey = activeQuery ? `${activeQuery}\u0000${urlProject}\u0000${runId}` : ''
  const [seenKey, setSeenKey] = useState('')
  if (seenKey !== requestKey) {
    setSeenKey(requestKey)
    setState(requestKey ? (prev) => ({ ...prev, loading: true, error: null }) : IDLE)
  }

  const projectOptions = useMemo<SelectProps.Option[]>(() => {
    const names = urlProject && !projects.includes(urlProject) ? [urlProject, ...projects] : projects
    return [ALL_PROJECTS, ...names.map((p) => ({ label: p, value: p }))]
  }, [projects, urlProject])
  const selectedProject = projectOptions.find((o) => o.value === urlProject) ?? ALL_PROJECTS

  return (
    <Container
      header={
        <Header
          variant="h2"
          description="Finds notes by meaning (semantic search) and by title or path."
          counter={activeQuery && !state.loading && !state.error ? `(${state.results.length})` : undefined}
        >
          Search
        </Header>
      }
    >
      <SpaceBetween size="m">
        <form
          role="search"
          onSubmit={(e) => {
            e.preventDefault()
            submit()
          }}
        >
          <SpaceBetween direction="horizontal" size="xs">
            <div style={{ minWidth: 'min(480px, 70vw)' }}>
              <Input
                type="search"
                value={input}
                onChange={({ detail }) => setInput(detail.value)}
                placeholder="Search notes by meaning or name…"
                ariaLabel="Search notes"
              />
            </div>
            <div style={{ minWidth: 180 }}>
              <Select
                selectedOption={selectedProject}
                onChange={({ detail }) => updateParams({ project: detail.selectedOption.value ?? '' }, true)}
                options={projectOptions}
                filteringType={projectOptions.length > 8 ? 'auto' : 'none'}
                ariaLabel="Project"
              />
            </div>
            {/* Submits the form; Enter in the input does too. */}
            <Button formAction="submit" disabled={input.trim() === ''}>
              Search
            </Button>
          </SpaceBetween>
        </form>

        {activeQuery &&
          (state.loading ? (
            <Box textAlign="center" padding="s">
              <Spinner /> Searching…
            </Box>
          ) : state.error ? (
            <Alert
              type="error"
              header="Search failed"
              action={<Button onClick={() => setRunId((n) => n + 1)}>Retry</Button>}
            >
              {state.error}
            </Alert>
          ) : state.results.length === 0 ? (
            <Box textAlign="center" color="inherit" padding="s">
              <b>No matching notes</b>
              <Box variant="p" color="text-body-secondary">
                Recently saved notes can take a moment to become searchable by meaning.
              </Box>
            </Box>
          ) : (
            <ul style={{ margin: 0, padding: 0 }} aria-label="Search results">
              {state.results.map((r) => (
                <ResultItem key={r.path} result={r} query={activeQuery} />
              ))}
            </ul>
          ))}
      </SpaceBetween>
    </Container>
  )
}
