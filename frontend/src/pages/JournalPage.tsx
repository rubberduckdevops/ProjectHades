import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import Autosuggest from '@cloudscape-design/components/autosuggest'
import Box from '@cloudscape-design/components/box'
import Button from '@cloudscape-design/components/button'
import Container from '@cloudscape-design/components/container'
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
import { formatLongDate, formatTime, todayRange } from '../lib/dates'
import { journalToMarkdown } from '../lib/markdown'
import { absorbProjects, loadRecentProjects, rememberProject } from '../lib/projects'
import { useSettings } from '../settings-store'

function sortByTimestamp(entries: JournalEntry[]): JournalEntry[] {
  return [...entries].sort((a, b) => a.timestamp.localeCompare(b.timestamp))
}

function projectsIn(entries: JournalEntry[]): string[] {
  return entries.flatMap((e) => (e.project ? [e.project] : []))
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

export default function JournalPage() {
  const [entries, setEntries] = useState<JournalEntry[]>([])
  const [loading, setLoading] = useState(true)
  const [input, setInput] = useState('')
  const [project, setProject] = useState('')
  const [recentProjects, setRecentProjects] = useState(loadRecentProjects)
  const bodyRef = useRef<TextareaProps.Ref>(null)
  const [submitting, setSubmitting] = useState(false)
  const [flash, setFlash] = useState<FlashbarProps.MessageDefinition[]>([])
  const flashId = useRef(0)

  const { timeZone } = useSettings()
  const [mountedAt] = useState(() => new Date())
  const todayLabel = formatLongDate(mountedAt, timeZone)

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

  const fetchEntries = useCallback(async () => {
    try {
      const { from, to } = todayRange(new Date(), timeZone)
      const result = await listJournal(from, to)
      setEntries(sortByTimestamp(result))
      setRecentProjects(absorbProjects(projectsIn(result)))
    } catch (err) {
      pushError('Failed to load entries', err)
    } finally {
      setLoading(false)
    }
  }, [pushError, timeZone])

  useEffect(() => {
    let cancelled = false
    const { from, to } = todayRange(new Date(), timeZone)
    listJournal(from, to)
      .then((result) => {
        if (!cancelled) {
          setEntries(sortByTimestamp(result))
          setRecentProjects(absorbProjects(projectsIn(result)))
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) pushError('Failed to load entries', err)
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [pushError, timeZone])

  const markdown = journalToMarkdown(entries, mountedAt, timeZone)
  const noEntries = loading || entries.length === 0

  const refresh = () => {
    setLoading(true)
    void fetchEntries()
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
      setEntries((prev) => sortByTimestamp([...prev.filter((e) => e.id !== entry.id), entry]))
      if (entry.project) setRecentProjects(rememberProject(entry.project))
      setInput('')
      setProject('')
    } catch (err) {
      pushError('Failed to add entry', err)
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <ContentLayout header={<Header variant="h1" description={todayLabel}>Journal</Header>}>
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
              cell: (item) => <span style={{ whiteSpace: 'pre-wrap' }}>{item.body}</span>,
            },
          ]}
          empty={
            <Box textAlign="center" color="inherit" padding="m">
              <b>No entries yet today</b>
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
              Today&apos;s entries
            </Header>
          }
        />
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
                    empty="No recent projects"
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
                  placeholder="What's on your mind?"
                  rows={3}
                  disabled={submitting}
                  ariaLabel="New journal entry"
                />
              </SpaceBetween>
            </Container>
          </Form>
        </form>
      </SpaceBetween>
    </ContentLayout>
  )
}
