import { useCallback, useEffect, useMemo, useState } from 'react'
import { useCollection } from '@cloudscape-design/collection-hooks'
import Badge from '@cloudscape-design/components/badge'
import Box from '@cloudscape-design/components/box'
import Button from '@cloudscape-design/components/button'
import ContentLayout from '@cloudscape-design/components/content-layout'
import Flashbar, { type FlashbarProps } from '@cloudscape-design/components/flashbar'
import Header from '@cloudscape-design/components/header'
import Link from '@cloudscape-design/components/link'
import Pagination from '@cloudscape-design/components/pagination'
import SpaceBetween from '@cloudscape-design/components/space-between'
import StatusIndicator from '@cloudscape-design/components/status-indicator'
import Table, { type TableProps } from '@cloudscape-design/components/table'
import TextFilter from '@cloudscape-design/components/text-filter'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { listNotes, type NoteMeta } from '../api/notes'
import NotesSearch from '../components/NotesSearch'
import { formatDateTime } from '../lib/dates'
import { encodeNotePath, folderOf } from '../lib/notePaths'
import { projectsOf } from '../lib/search'
import { useSettings } from '../settings-store'

const PAGE_SIZE = 50

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

function matches(note: NoteMeta, filteringText: string): boolean {
  const needle = filteringText.trim().toLowerCase()
  if (!needle) return true
  return [note.title, note.path, note.project ?? '', ...note.tags].some((field) =>
    field.toLowerCase().includes(needle),
  )
}

const updatedColumn: TableProps.SortingColumn<NoteMeta> = { sortingField: 'updatedAt' }

export default function NotesPage() {
  const navigate = useNavigate()
  const [searchParams] = useSearchParams()
  const { timeZone } = useSettings()
  const [notes, setNotes] = useState<NoteMeta[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      setNotes(await listNotes())
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    listNotes()
      .then((result) => {
        if (!cancelled) setNotes(result)
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(errorMessage(err))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [])

  const openNote = (path: string) => navigate(`/notes/${encodeNotePath(path)}`)
  const newNote = () => navigate('/notes/new')

  const { items, filteredItemsCount, collectionProps, filterProps, paginationProps } = useCollection(notes, {
    filtering: {
      defaultFilteringText: searchParams.get('filter') ?? '',
      filteringFunction: matches,
      empty: (
        <Box textAlign="center" color="inherit" padding="m">
          <SpaceBetween size="xs">
            <b>No notes yet</b>
            <Button onClick={newNote}>New note</Button>
          </SpaceBetween>
        </Box>
      ),
      noMatch: (
        <Box textAlign="center" color="inherit" padding="m">
          <b>No matching notes</b>
        </Box>
      ),
    },
    sorting: { defaultState: { sortingColumn: updatedColumn, isDescending: true } },
    pagination: { pageSize: PAGE_SIZE },
  })

  const flash: FlashbarProps.MessageDefinition[] = error
    ? [
        {
          id: 'load-error',
          type: 'error',
          header: 'Failed to load notes',
          content: error,
          dismissible: true,
          dismissLabel: 'Dismiss',
          onDismiss: () => setError(null),
        },
      ]
    : []

  const total = filteredItemsCount ?? notes.length
  const projects = useMemo(() => projectsOf(notes), [notes])

  return (
    <ContentLayout
      header={
        <Header
          variant="h1"
          actions={
            <Button variant="primary" onClick={newNote}>
              New note
            </Button>
          }
        >
          Notes
        </Header>
      }
    >
      <SpaceBetween size="l">
        {flash.length > 0 && <Flashbar items={flash} />}
        <NotesSearch projects={projects} />
        <Table
          {...collectionProps}
          variant="container"
          items={items}
          loading={loading}
          loadingText="Loading notes"
          trackBy="path"
          wrapLines
          columnDefinitions={[
            {
              id: 'title',
              header: 'Title',
              sortingField: 'title',
              cell: (note) => (
                <SpaceBetween size="xxxs">
                  <Link
                    href={`/notes/${encodeNotePath(note.path)}`}
                    onFollow={(e) => {
                      e.preventDefault()
                      openNote(note.path)
                    }}
                  >
                    {note.title}
                  </Link>
                  {note.indexed === false && (
                    <Box variant="small">
                      <StatusIndicator type="pending">Indexing…</StatusIndicator>
                    </Box>
                  )}
                </SpaceBetween>
              ),
            },
            {
              id: 'folder',
              header: 'Folder',
              sortingComparator: (a, b) => folderOf(a.path).localeCompare(folderOf(b.path)),
              cell: (note) => folderOf(note.path) || <Box color="text-status-inactive">/</Box>,
            },
            {
              id: 'project',
              header: 'Project',
              sortingComparator: (a, b) => (a.project ?? '').localeCompare(b.project ?? ''),
              cell: (note) => note.project ?? <Box color="text-status-inactive">-</Box>,
            },
            {
              id: 'tags',
              header: 'Tags',
              cell: (note) =>
                note.tags.length > 0 ? (
                  <SpaceBetween direction="horizontal" size="xxs">
                    {note.tags.map((tag) => (
                      <Badge key={tag}>{tag}</Badge>
                    ))}
                  </SpaceBetween>
                ) : (
                  <Box color="text-status-inactive">-</Box>
                ),
            },
            {
              id: 'updated',
              header: 'Updated',
              sortingField: updatedColumn.sortingField,
              cell: (note) => formatDateTime(note.updatedAt, timeZone),
              width: 200,
            },
          ]}
          filter={
            <TextFilter
              {...filterProps}
              filteringPlaceholder="Filter by title, path, project or tag"
              filteringAriaLabel="Filter notes"
              countText={`${total} ${total === 1 ? 'match' : 'matches'}`}
            />
          }
          pagination={total > PAGE_SIZE ? <Pagination {...paginationProps} /> : undefined}
          header={
            <Header
              counter={loading ? undefined : `(${notes.length})`}
              actions={
                <Button iconName="refresh" onClick={() => void load()} disabled={loading} ariaLabel="Refresh">
                  Refresh
                </Button>
              }
            >
              All notes
            </Header>
          }
        />
      </SpaceBetween>
    </ContentLayout>
  )
}
