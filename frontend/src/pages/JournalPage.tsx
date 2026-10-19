import { useCallback, useEffect, useRef, useState } from 'react'
import Box from '@cloudscape-design/components/box'
import Button from '@cloudscape-design/components/button'
import Container from '@cloudscape-design/components/container'
import CopyToClipboard from '@cloudscape-design/components/copy-to-clipboard'
import ContentLayout from '@cloudscape-design/components/content-layout'
import Flashbar, { type FlashbarProps } from '@cloudscape-design/components/flashbar'
import Form from '@cloudscape-design/components/form'
import Header from '@cloudscape-design/components/header'
import SpaceBetween from '@cloudscape-design/components/space-between'
import Table from '@cloudscape-design/components/table'
import Textarea from '@cloudscape-design/components/textarea'
import { listJournal, upsertJournal, type JournalEntry } from '../api/client'
import { formatLongDate, formatTime, todayRange } from '../lib/dates'
import { journalToMarkdown } from '../lib/markdown'
import { useSettings } from '../settings-store'

function sortByTimestamp(entries: JournalEntry[]): JournalEntry[] {
  return [...entries].sort((a, b) => a.timestamp.localeCompare(b.timestamp))
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

export default function JournalPage() {
  const [entries, setEntries] = useState<JournalEntry[]>([])
  const [loading, setLoading] = useState(true)
  const [input, setInput] = useState('')
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
        if (!cancelled) setEntries(sortByTimestamp(result))
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

  const trimmed = input.trim()
  const canSubmit = trimmed.length > 0 && !submitting

  const submit = async () => {
    if (!canSubmit) return
    setSubmitting(true)
    try {
      const entry = await upsertJournal({ body: trimmed })
      setEntries((prev) => sortByTimestamp([...prev.filter((e) => e.id !== entry.id), entry]))
      setInput('')
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
              <Textarea
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
            </Container>
          </Form>
        </form>
      </SpaceBetween>
    </ContentLayout>
  )
}
