import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import Autosuggest from '@cloudscape-design/components/autosuggest'
import Box from '@cloudscape-design/components/box'
import Button from '@cloudscape-design/components/button'
import Container from '@cloudscape-design/components/container'
import DatePicker from '@cloudscape-design/components/date-picker'
import CopyToClipboard from '@cloudscape-design/components/copy-to-clipboard'
import ContentLayout from '@cloudscape-design/components/content-layout'
import Flashbar, { type FlashbarProps } from '@cloudscape-design/components/flashbar'
import Form from '@cloudscape-design/components/form'
import FormField from '@cloudscape-design/components/form-field'
import Header from '@cloudscape-design/components/header'
import SpaceBetween from '@cloudscape-design/components/space-between'
import Table from '@cloudscape-design/components/table'
import Textarea, { type TextareaProps } from '@cloudscape-design/components/textarea'
import { listJournal, upsertJournal, type JournalEntry } from '../api/client'
import { addDays, calendarDate, dayRange, formatLongDate, formatTime, isCalendarDate } from '../lib/dates'
import { journalToMarkdown } from '../lib/markdown'
import { useProjects } from '../lib/useProjects'
import { useSettings } from '../settings-store'

// Loaded on demand so react-markdown stays out of the initial bundle.
const MarkdownPreview = lazy(() => import('../components/MarkdownPreview'))

function EntryBody({ body }: { body: string }) {
  const plain = <span style={{ whiteSpace: 'pre-wrap' }}>{body}</span>
  return (
    <Suspense fallback={plain}>
      <MarkdownPreview content={body} compact />
    </Suspense>
  )
}

function sortByTimestamp(entries: JournalEntry[]): JournalEntry[] {
  return [...entries].sort((a, b) => a.timestamp.localeCompare(b.timestamp))
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

interface LoadedDay {
  day: string
  entries: JournalEntry[]
}

export default function JournalPage() {
  const [loaded, setLoaded] = useState<LoadedDay | null>(null)
  const [refreshing, setRefreshing] = useState(false)
  const [input, setInput] = useState('')
  const [project, setProject] = useState('')
  const bodyRef = useRef<TextareaProps.Ref>(null)
  const [submitting, setSubmitting] = useState(false)
  const [flash, setFlash] = useState<FlashbarProps.MessageDefinition[]>([])
  const flashId = useRef(0)

  const { timeZone } = useSettings()
  const [mountedAt] = useState(() => new Date())
  const today = calendarDate(mountedAt, timeZone)
  // ?date=YYYY-MM-DD picks a past day; anything else (or a future date) means today.
  const [searchParams, setSearchParams] = useSearchParams()
  const requested = searchParams.get('date') ?? ''
  const day = isCalendarDate(requested) && requested < today ? requested : today
  const isToday = day === today
  const range = useMemo(() => dayRange(day, timeZone), [day, timeZone])
  const dayStart = useMemo(() => new Date(range.from), [range])

  const entries = loaded?.day === day ? loaded.entries : []
  const loading = loaded?.day !== day || refreshing

  const goTo = (next: string) => {
    setSearchParams(next >= today ? {} : { date: next })
  }

  const pushError = useCallback((header: string, err: unknown) => {
    const id = `err-${++flashId.current}`
    setFlash((prev) => [
      ...prev,
      {
        id,
        type: 'error',
        header,
        content: errorMessage(err),
        dismissible: true,
        dismissLabel: 'Dismiss',
        onDismiss: () => setFlash((items) => items.filter((item) => item.id !== id)),
      },
    ])
  }, [])

  const onProjectsError = useCallback((err: unknown) => pushError('Failed to load projects', err), [pushError])
  const { projects: recentProjects, remember: rememberProject } = useProjects(onProjectsError)

  const load = useCallback(
    (isCancelled: () => boolean = () => false) =>
      listJournal(range.from, range.to)
        .then((result) => {
          if (!isCancelled()) setLoaded({ day, entries: sortByTimestamp(result) })
        })
        .catch((err: unknown) => {
          if (isCancelled()) return
          pushError('Failed to load entries', err)
          setLoaded({ day, entries: [] })
        }),
    [day, range, pushError],
  )

  useEffect(() => {
    let cancelled = false
    void load(() => cancelled)
    return () => {
      cancelled = true
    }
  }, [load])

  const markdown = journalToMarkdown(entries, dayStart, timeZone)
  const noEntries = loading || entries.length === 0

  const refresh = () => {
    setRefreshing(true)
    void load().finally(() => setRefreshing(false))
  }

  const projectOptions = useMemo(() => recentProjects.map((value) => ({ value })), [recentProjects])

  const trimmed = input.trim()
  const canSubmit = trimmed.length > 0 && !submitting

  const submit = async () => {
    if (!canSubmit) return
    setSubmitting(true)
    try {
      const projectName = project.trim()
      // undefined is dropped by JSON.stringify, so a blank project is simply omitted.
      const entry = await upsertJournal({ body: trimmed, project: projectName || undefined })
      setLoaded((prev) =>
        prev && prev.day === today
          ? { day: today, entries: sortByTimestamp([...prev.entries.filter((e) => e.id !== entry.id), entry]) }
          : prev,
      )
      if (entry.project) rememberProject(entry.project)
      setInput('')
      setProject('')
    } catch (err) {
      pushError('Failed to add entry', err)
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <ContentLayout
      header={
        <Header
          variant="h1"
          description={formatLongDate(dayStart, timeZone)}
          actions={
            <SpaceBetween direction="horizontal" size="xs" alignItems="center">
              <Button iconName="angle-left" onClick={() => goTo(addDays(day, -1))} ariaLabel="Previous day">
                Previous
              </Button>
              <DatePicker
                value={day}
                onChange={({ detail }) => {
                  if (isCalendarDate(detail.value)) goTo(detail.value)
                }}
                isDateEnabled={(date) => calendarDate(date, timeZone) <= today}
                openCalendarAriaLabel={(selected) => (selected ? `Change day, ${selected}` : 'Choose day')}
                placeholder="YYYY/MM/DD"
              />
              <Button
                iconName="angle-right"
                iconAlign="right"
                onClick={() => goTo(addDays(day, 1))}
                disabled={isToday}
                ariaLabel="Next day"
              >
                Next
              </Button>
              <Button onClick={() => goTo(today)} disabled={isToday}>
                Today
              </Button>
            </SpaceBetween>
          }
        >
          Journal
        </Header>
      }
    >
      <SpaceBetween size="l">
        {flash.length > 0 && <Flashbar items={flash} />}
        <Table
          variant="container"
          items={entries}
          loading={loading}
          loadingText="Loading entries"
          trackBy="id"
          wrapLines
          columnDefinitions={[
            {
              id: 'time',
              header: 'Time',
              cell: (item) => formatTime(item.timestamp, timeZone),
              width: 120,
            },
            {
              id: 'project',
              header: 'Project',
              cell: (item) => item.project ?? <Box color="text-status-inactive">General</Box>,
              width: 160,
            },
            {
              id: 'body',
              header: 'Entry',
              cell: (item) => <EntryBody body={item.body} />,
            },
          ]}
          empty={
            <Box textAlign="center" color="inherit" padding="m">
              <b>{isToday ? 'No entries yet today' : 'No entries on this day'}</b>
            </Box>
          }
          header={
            <Header
              counter={`(${entries.length})`}
              actions={
                <SpaceBetween direction="horizontal" size="xs">
                  <CopyToClipboard
                    variant="button"
                    textToCopy={markdown}
                    copyButtonText="Copy as Markdown"
                    copySuccessText="Markdown copied"
                    copyErrorText="Couldn't copy"
                    disabled={noEntries}
                  />
                  <Button iconName="refresh" onClick={refresh} disabled={loading} ariaLabel="Refresh">
                    Refresh
                  </Button>
                </SpaceBetween>
              }
            >
              {isToday ? <>Today&apos;s entries</> : 'Entries'}
            </Header>
          }
        />
        {isToday && (
          <form onSubmit={(e) => e.preventDefault()}>
            <Form
              actions={
                <Button
                  variant="primary"
                  onClick={() => void submit()}
                  disabled={!canSubmit}
                  loading={submitting}
                >
                  Add
                </Button>
              }
            >
              <Container header={<Header variant="h2">New entry</Header>}>
                <SpaceBetween size="s">
                  <FormField label="Project" description="Optional. Leave blank for a general entry.">
                    <Autosuggest
                      value={project}
                      onChange={({ detail }) => setProject(detail.value)}
                      onSelect={() => bodyRef.current?.focus()}
                      onKeyDown={({ detail }) => {
                        if (detail.key === 'Enter') bodyRef.current?.focus()
                      }}
                      options={projectOptions}
                      enteredTextLabel={(value) => `Use "${value}"`}
                      placeholder="General"
                      empty="No projects yet"
                      disabled={submitting}
                      ariaLabel="Project"
                    />
                  </FormField>
                  <Textarea
                    ref={bodyRef}
                    value={input}
                    onChange={({ detail }) => setInput(detail.value)}
                    onKeyDown={(event) => {
                      if (event.detail.key === 'Enter' && !event.detail.shiftKey) {
                        event.preventDefault()
                        void submit()
                      }
                    }}
                    placeholder="What's on your mind? Markdown works: **bold**, `code`, [links](https://…), ``` blocks."
                    rows={3}
                    disabled={submitting}
                    ariaLabel="New journal entry"
                  />
                </SpaceBetween>
              </Container>
            </Form>
          </form>
        )}
      </SpaceBetween>
    </ContentLayout>
  )
}
