import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from 'react'
import Alert from '@cloudscape-design/components/alert'
import Autosuggest from '@cloudscape-design/components/autosuggest'
import Badge from '@cloudscape-design/components/badge'
import Box from '@cloudscape-design/components/box'
import BreadcrumbGroup, { type BreadcrumbGroupProps } from '@cloudscape-design/components/breadcrumb-group'
import Button from '@cloudscape-design/components/button'
import Container from '@cloudscape-design/components/container'
import ContentLayout from '@cloudscape-design/components/content-layout'
import CopyToClipboard from '@cloudscape-design/components/copy-to-clipboard'
import Flashbar, { type FlashbarProps } from '@cloudscape-design/components/flashbar'
import Form from '@cloudscape-design/components/form'
import FormField from '@cloudscape-design/components/form-field'
import Header from '@cloudscape-design/components/header'
import Input from '@cloudscape-design/components/input'
import KeyValuePairs from '@cloudscape-design/components/key-value-pairs'
import Modal from '@cloudscape-design/components/modal'
import SpaceBetween from '@cloudscape-design/components/space-between'
import Spinner from '@cloudscape-design/components/spinner'
import StatusIndicator from '@cloudscape-design/components/status-indicator'
import Tabs from '@cloudscape-design/components/tabs'
import Textarea from '@cloudscape-design/components/textarea'
import { fontFamilyMonospace } from '@cloudscape-design/design-tokens'
import { useLocation, useNavigate, useSearchParams, type NavigateFunction } from 'react-router-dom'
import { ApiError } from '../api/client'
import { createNote, deleteNote, getNote, listNotes, moveNote, updateNote, type Note, type NoteMeta } from '../api/notes'
import { useUnsavedChangesGuard } from '../components/UnsavedChangesGuard'
import { formatDateTime } from '../lib/dates'
import {
  decodeNotePath,
  defaultPathFor,
  encodeNotePath,
  fileNameOf,
  foldersOf,
  validateNotePath,
} from '../lib/notePaths'
import { useSettings } from '../settings-store'

// Loaded on first preview so react-markdown stays out of the initial bundle.
const MarkdownPreview = lazy(() => import('../components/MarkdownPreview'))

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

const isStatus = (err: unknown, status: number) => err instanceof ApiError && err.status === status

const editorUrl = (path: string) => `/notes/${encodeNotePath(path)}`

/** Calls `onSave` on Ctrl/Cmd+S anywhere on the page, suppressing the browser's save dialog. */
function useSaveShortcut(onSave: () => void) {
  const ref = useRef(onSave)
  useEffect(() => {
    ref.current = onSave
  }, [onSave])
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 's') {
        e.preventDefault()
        ref.current()
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [])
}

function useFlash() {
  const [items, setItems] = useState<FlashbarProps.MessageDefinition[]>([])
  const nextId = useRef(0)
  const dismiss = useCallback((id: string) => setItems((prev) => prev.filter((i) => i.id !== id)), [])
  const push = useCallback(
    (item: Omit<FlashbarProps.MessageDefinition, 'id'>, autoDismissMs?: number) => {
      const id = `flash-${++nextId.current}`
      setItems((prev) => [
        ...prev,
        { ...item, id, dismissible: true, dismissLabel: 'Dismiss', onDismiss: () => dismiss(id) },
      ])
      if (autoDismissMs) setTimeout(() => dismiss(id), autoDismissMs)
    },
    [dismiss],
  )
  const clear = useCallback(() => setItems([]), [])
  return { items, push, clear }
}

interface BodyEditorProps {
  value: string
  onChange: (value: string) => void
  disabled?: boolean
}

function NoteBodyEditor({ value, onChange, disabled }: BodyEditorProps) {
  const [tab, setTab] = useState('edit')
  return (
    <Tabs
      activeTabId={tab}
      onChange={({ detail }) => setTab(detail.activeTabId)}
      tabs={[
        {
          id: 'edit',
          label: 'Edit',
          content: (
            <div style={{ fontFamily: fontFamilyMonospace }}>
              <Textarea
                value={value}
                onChange={({ detail }) => onChange(detail.value)}
                rows={25}
                spellcheck={false}
                disabled={disabled}
                placeholder="Write Markdown here. Optional frontmatter (title:, project:, tags:) goes between --- lines at the top."
                ariaLabel="Note content (Markdown)"
              />
            </div>
          ),
        },
        {
          id: 'preview',
          label: 'Preview',
          content: (
            <Suspense fallback={<Spinner />}>
              <MarkdownPreview content={value} />
            </Suspense>
          ),
        },
      ]}
    />
  )
}

function noteBreadcrumbs(path: string | null, navigate: NavigateFunction): BreadcrumbGroupProps {
  const items: BreadcrumbGroupProps.Item[] = [{ text: 'Notes', href: '/notes' }]
  if (path === null) {
    items.push({ text: 'New note', href: '/notes/new' })
  } else {
    const segments = path.split('/')
    segments.slice(0, -1).forEach((segment, i) => {
      const prefix = `${segments.slice(0, i + 1).join('/')}/`
      items.push({ text: segment, href: `/notes?filter=${encodeURIComponent(prefix)}` })
    })
    items.push({ text: fileNameOf(path), href: editorUrl(path) })
  }
  return {
    items,
    ariaLabel: 'Breadcrumbs',
    onFollow: (e) => {
      e.preventDefault()
      navigate(e.detail.href)
    },
  }
}

// ---------------------------------------------------------------------------
// New note
// ---------------------------------------------------------------------------

function NewNoteEditor() {
  const navigate = useNavigate()
  const [searchParams] = useSearchParams()
  const initialPath = searchParams.get('path') ?? ''
  const [title, setTitle] = useState('')
  const [folder, setFolder] = useState(searchParams.get('folder') ?? '')
  const [customPath, setCustomPath] = useState<string | null>(initialPath || null)
  const [content, setContent] = useState('')
  const [folders, setFolders] = useState<string[]>([])
  const [creating, setCreating] = useState(false)
  const [pathError, setPathError] = useState<string | null>(null)
  const [showErrors, setShowErrors] = useState(false)
  const flash = useFlash()

  useEffect(() => {
    let cancelled = false
    listNotes()
      .then((notes) => {
        if (!cancelled) setFolders(foldersOf(notes.map((n) => n.path)))
      })
      .catch(() => {
        // Folder suggestions are a nicety; ignore failures.
      })
    return () => {
      cancelled = true
    }
  }, [])

  const path = customPath ?? defaultPathFor(folder, title)
  const validation = validateNotePath(path)
  const dirty = title !== '' || content !== '' || customPath !== null
  const guard = useUnsavedChangesGuard(dirty && !creating)

  const create = async () => {
    if (creating) return
    setShowErrors(true)
    if (validation) return
    setCreating(true)
    setPathError(null)
    try {
      // The server titles a note from frontmatter, else its first H1, else the file name.
      // If the path was edited so the file name no longer matches the title, seed an H1
      // (unless the body already has frontmatter or an H1) so the entered title sticks.
      const wanted = title.trim()
      const stem = fileNameOf(path).replace(/\.md$/, '')
      const hasTitle = content.startsWith('---') || /^# /m.test(content)
      const body = wanted && wanted !== stem && !hasTitle ? `# ${wanted}\n\n${content}` : content
      const meta = await createNote(path, body)
      guard.allowNextNavigation()
      navigate(editorUrl(meta.path), { replace: true })
    } catch (err) {
      if (isStatus(err, 409)) {
        setPathError('A note already exists at this path. Choose a different title, folder or path.')
      } else if (isStatus(err, 400)) {
        setPathError(errorMessage(err))
      } else {
        flash.push({ type: 'error', header: 'Failed to create note', content: errorMessage(err) })
      }
      setCreating(false)
    }
  }

  useSaveShortcut(() => void create())

  const folderOptions = useMemo(() => folders.map((value) => ({ value })), [folders])
  const shownPathError = pathError ?? (showErrors || customPath !== null || title !== '' ? validation : null)

  return (
    <ContentLayout
      header={
        <SpaceBetween size="xs">
          <BreadcrumbGroup {...noteBreadcrumbs(null, navigate)} />
          <Header variant="h1" description="The note is created when you press Create (or Ctrl/Cmd+S).">
            New note
          </Header>
        </SpaceBetween>
      }
    >
      {guard.modal}
      <form onSubmit={(e) => e.preventDefault()}>
        <Form
          actions={
            <SpaceBetween direction="horizontal" size="xs">
              <Button variant="link" onClick={() => navigate('/notes')}>
                Cancel
              </Button>
              <Button variant="primary" onClick={() => void create()} loading={creating}>
                Create
              </Button>
            </SpaceBetween>
          }
        >
          <SpaceBetween size="l">
            {flash.items.length > 0 && <Flashbar items={flash.items} />}
            <Container header={<Header variant="h2">Location</Header>}>
              <SpaceBetween size="m">
                <FormField label="Title">
                  <Input
                    value={title}
                    onChange={({ detail }) => setTitle(detail.value)}
                    placeholder="Design"
                    autoFocus
                    disabled={creating}
                  />
                </FormField>
                <FormField label="Folder" description="Optional. Use / for nested folders.">
                  <Autosuggest
                    value={folder}
                    onChange={({ detail }) => setFolder(detail.value)}
                    options={folderOptions}
                    enteredTextLabel={(value) => `Use "${value}"`}
                    placeholder="Projects/Hades"
                    empty="No existing folders"
                    disabled={creating}
                    ariaLabel="Folder"
                  />
                </FormField>
                <FormField
                  label="Path"
                  description={
                    customPath === null
                      ? 'Derived from folder and title. Edit to override.'
                      : 'Edited manually.'
                  }
                  errorText={shownPathError ?? undefined}
                  secondaryControl={
                    customPath !== null ? (
                      <Button variant="normal" onClick={() => setCustomPath(null)} disabled={creating}>
                        Reset
                      </Button>
                    ) : undefined
                  }
                >
                  <Input
                    value={path}
                    onChange={({ detail }) => {
                      setCustomPath(detail.value)
                      setPathError(null)
                    }}
                    invalid={!!shownPathError}
                    disabled={creating}
                    ariaLabel="Path"
                  />
                </FormField>
              </SpaceBetween>
            </Container>
            <Container header={<Header variant="h2">Content</Header>}>
              <NoteBodyEditor value={content} onChange={setContent} disabled={creating} />
            </Container>
          </SpaceBetween>
        </Form>
      </form>
    </ContentLayout>
  )
}

// ---------------------------------------------------------------------------
// Existing note
// ---------------------------------------------------------------------------

const INDEX_POLL_MS = 3000
const INDEX_POLL_MAX_MS = 60_000

/**
 * While `meta.indexed === false`, re-check the note's metadata every few seconds (up to a
 * minute) and flip `indexed` to true once the server reports the *same*
 * contentHash as indexed. Only `indexed` is updated: editor content and the
 * base hash are never touched. Stops on unmount, path change or a new hash.
 */
function useIndexPolling(path: string, meta: NoteMeta | null, setMeta: Dispatch<SetStateAction<NoteMeta | null>>) {
  const hash = meta?.contentHash
  const pending = meta?.indexed === false
  useEffect(() => {
    if (!pending || !hash) return
    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const deadline = Date.now() + INDEX_POLL_MAX_MS
    const tick = async () => {
      try {
        // Metadata only (no note body): list with the exact path as prefix, then pick that path.
        const latest = (await listNotes(path)).find((n) => n.path === path)
        if (!latest) return
        if (cancelled) return
        if (latest.indexed && latest.contentHash === hash) {
          setMeta((m) => (m && m.contentHash === hash ? { ...m, indexed: true } : m))
          return
        }
      } catch {
        if (cancelled) return
        // Transient failure: keep trying until the deadline.
      }
      if (Date.now() + INDEX_POLL_MS <= deadline) timer = setTimeout(() => void tick(), INDEX_POLL_MS)
    }
    timer = setTimeout(() => void tick(), INDEX_POLL_MS)
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
  }, [path, hash, pending, setMeta])
}

function IndexStatus({ indexed }: { indexed: boolean | undefined }) {
  if (indexed === undefined) return <>-</>
  return indexed ? (
    <StatusIndicator type="success">Search index up to date</StatusIndicator>
  ) : (
    <StatusIndicator type="pending">Indexing… new changes will be searchable shortly</StatusIndicator>
  )
}

interface Conflict {
  currentHash?: string
}

function ExistingNoteEditor({ path }: { path: string }) {
  const navigate = useNavigate()
  const { timeZone } = useSettings()
  const flash = useFlash()

  const [meta, setMeta] = useState<NoteMeta | null>(null)
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState<unknown>(null)
  const [content, setContent] = useState('')
  const [savedContent, setSavedContent] = useState('')
  const [saving, setSaving] = useState(false)
  const [conflict, setConflict] = useState<Conflict | null>(null)
  const [resolving, setResolving] = useState(false)

  const [moveOpen, setMoveOpen] = useState(false)
  const [moveTarget, setMoveTarget] = useState(path)
  const [moveError, setMoveError] = useState<string | null>(null)
  const [moving, setMoving] = useState(false)

  const [deleteOpen, setDeleteOpen] = useState(false)
  const [deleting, setDeleting] = useState(false)

  const dirty = meta !== null && content !== savedContent
  const guard = useUnsavedChangesGuard(dirty)
  useIndexPolling(path, meta, setMeta)

  const applyLoaded = useCallback((note: Note) => {
    setMeta(note)
    setContent(note.content)
    setSavedContent(note.content)
  }, [])

  useEffect(() => {
    let cancelled = false
    getNote(path)
      .then((note) => {
        if (!cancelled) applyLoaded(note)
      })
      .catch((err: unknown) => {
        if (!cancelled) setLoadError(err)
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [path, applyLoaded])

  const reload = async (): Promise<boolean> => {
    try {
      applyLoaded(await getNote(path))
      return true
    } catch (err) {
      flash.push({ type: 'error', header: 'Failed to reload note', content: errorMessage(err) })
      return false
    }
  }

  const afterSave = (saved: string, next: NoteMeta) => {
    setMeta(next)
    setSavedContent(saved)
    flash.push({ type: 'success', content: 'Saved' }, 3000)
  }

  const recreate = async (snapshot: string) => {
    try {
      afterSave(snapshot, await createNote(path, snapshot))
    } catch (err) {
      flash.push({ type: 'error', header: 'Failed to recreate note', content: errorMessage(err) })
    }
  }

  const save = async (baseHash?: string) => {
    if (!meta || saving) return
    const snapshot = content
    setSaving(true)
    try {
      afterSave(snapshot, await updateNote(path, snapshot, baseHash ?? meta.contentHash))
      setConflict(null)
    } catch (err) {
      if (isStatus(err, 409)) {
        setConflict({ currentHash: (err as ApiError).currentHash })
      } else if (isStatus(err, 404)) {
        setConflict(null)
        flash.push({
          type: 'error',
          header: 'This note no longer exists',
          content: 'It was deleted or moved elsewhere. Recreate it with your content, or copy your text first.',
          action: <Button onClick={() => void recreate(snapshot)}>Recreate</Button>,
        })
      } else {
        flash.push({ type: 'error', header: 'Failed to save', content: errorMessage(err) })
      }
    } finally {
      setSaving(false)
    }
  }

  useSaveShortcut(() => {
    if (dirty && !conflict) void save()
  })

  const resolveReload = async () => {
    setResolving(true)
    if (await reload()) setConflict(null)
    setResolving(false)
  }

  const resolveOverwrite = async () => {
    if (!conflict?.currentHash) return
    setResolving(true)
    await save(conflict.currentHash)
    setResolving(false)
  }

  const openMove = () => {
    setMoveTarget(path)
    setMoveError(null)
    setMoveOpen(true)
  }

  const moveValidation = moveTarget === path ? 'Enter a different path' : validateNotePath(moveTarget)

  const doMove = async () => {
    if (!meta || moveValidation) return
    setMoving(true)
    setMoveError(null)
    try {
      const next = await moveNote(path, moveTarget, meta.contentHash)
      guard.allowNextNavigation()
      navigate(editorUrl(next.path), { replace: true })
    } catch (err) {
      if (isStatus(err, 409) && (err as ApiError).currentHash) {
        setMoveError('This note changed since you opened it. Close this dialog and reload or save first.')
      } else if (isStatus(err, 409)) {
        setMoveError('A note already exists at that path.')
      } else {
        setMoveError(errorMessage(err))
      }
      setMoving(false)
    }
  }

  const doDelete = async () => {
    if (!meta) return
    setDeleting(true)
    try {
      await deleteNote(path, meta.contentHash)
      guard.allowNextNavigation()
      navigate('/notes')
    } catch (err) {
      setDeleteOpen(false)
      setDeleting(false)
      if (isStatus(err, 409)) {
        const reloaded = await reload()
        flash.push({
          type: 'warning',
          header: 'Not deleted: the note changed since you opened it',
          content: reloaded
            ? 'The latest version has been loaded. Review it and delete again if you still want to.'
            : 'Reload the page to see the latest version.',
        })
      } else if (isStatus(err, 404)) {
        flash.push({ type: 'info', content: 'This note was already deleted.' })
      } else {
        flash.push({ type: 'error', header: 'Failed to delete', content: errorMessage(err) })
      }
    }
  }

  const breadcrumbs = <BreadcrumbGroup {...noteBreadcrumbs(path, navigate)} />

  if (loading) {
    return (
      <ContentLayout header={<SpaceBetween size="xs">{breadcrumbs}<Header variant="h1">{fileNameOf(path)}</Header></SpaceBetween>}>
        <Box textAlign="center" padding="xl">
          <Spinner size="large" />
        </Box>
      </ContentLayout>
    )
  }

  if (!meta) {
    const notFound = isStatus(loadError, 404)
    return (
      <ContentLayout header={<SpaceBetween size="xs">{breadcrumbs}<Header variant="h1">{fileNameOf(path)}</Header></SpaceBetween>}>
        <Alert
          type={notFound ? 'info' : 'error'}
          header={notFound ? 'Note not found' : 'Failed to load note'}
          action={
            notFound ? (
              <Button onClick={() => navigate(`/notes/new?path=${encodeURIComponent(path)}`)}>Create it</Button>
            ) : undefined
          }
        >
          {notFound ? (
            <>
              There is no note at <code>{path}</code>. It may have been moved or deleted.
            </>
          ) : (
            errorMessage(loadError)
          )}
        </Alert>
      </ContentLayout>
    )
  }

  return (
    <ContentLayout
      header={
        <SpaceBetween size="xs">
          {breadcrumbs}
          <Header
            variant="h1"
            description={`Updated ${formatDateTime(meta.updatedAt, timeZone)}${dirty ? ' · Unsaved changes' : ''}`}
            actions={
              <SpaceBetween direction="horizontal" size="xs">
                <CopyToClipboard
                  variant="button"
                  textToCopy={content}
                  copyButtonText="Copy as Markdown"
                  copySuccessText="Markdown copied"
                  copyErrorText="Couldn't copy"
                />
                <Button onClick={openMove}>Move / rename</Button>
                <Button onClick={() => setDeleteOpen(true)}>Delete</Button>
                <Button
                  variant="primary"
                  onClick={() => void save()}
                  disabled={!dirty || saving}
                  loading={saving}
                  ariaLabel="Save (Ctrl+S)"
                >
                  Save
                </Button>
              </SpaceBetween>
            }
          >
            {meta.title}
          </Header>
        </SpaceBetween>
      }
    >
      {guard.modal}
      <SpaceBetween size="l">
        {flash.items.length > 0 && <Flashbar items={flash.items} />}
        <Container
          header={
            <Header variant="h2" description="Set via frontmatter: project:, tags:">
              Details
            </Header>
          }
        >
          <KeyValuePairs
            columns={4}
            items={[
              { label: 'Path', value: <CopyToClipboard variant="inline" textToCopy={meta.path} copySuccessText="Path copied" copyErrorText="Couldn't copy" /> },
              { label: 'Project', value: meta.project ?? '-' },
              {
                label: 'Tags',
                value:
                  meta.tags.length > 0 ? (
                    <SpaceBetween direction="horizontal" size="xxs">
                      {meta.tags.map((tag) => (
                        <Badge key={tag}>{tag}</Badge>
                      ))}
                    </SpaceBetween>
                  ) : (
                    '-'
                  ),
              },
              { label: 'Search index', value: <IndexStatus indexed={meta.indexed} /> },
            ]}
          />
        </Container>
        <Container>
          <NoteBodyEditor value={content} onChange={setContent} />
        </Container>
      </SpaceBetween>

      <Modal
        visible={conflict !== null}
        onDismiss={() => !resolving && setConflict(null)}
        header="This note changed since you opened it"
        footer={
          <Box float="right">
            <SpaceBetween direction="horizontal" size="xs">
              <Button variant="link" onClick={() => setConflict(null)} disabled={resolving}>
                Cancel
              </Button>
              <Button onClick={() => void resolveReload()} disabled={resolving}>
                Reload (discard my changes)
              </Button>
              <Button
                variant="primary"
                onClick={() => void resolveOverwrite()}
                disabled={resolving || !conflict?.currentHash}
                loading={resolving && saving}
              >
                Overwrite
              </Button>
            </SpaceBetween>
          </Box>
        }
      >
        <SpaceBetween size="m">
          <Box>
            Someone (or another tab or device) saved a different version of this note. You can reload the
            latest version and lose your edits, overwrite it with your version, or copy your version first.
          </Box>
          <CopyToClipboard
            variant="button"
            textToCopy={content}
            copyButtonText="Copy my version"
            copySuccessText="Your version was copied"
            copyErrorText="Couldn't copy"
          />
        </SpaceBetween>
      </Modal>

      <Modal
        visible={moveOpen}
        onDismiss={() => !moving && setMoveOpen(false)}
        header="Move / rename note"
        footer={
          <Box float="right">
            <SpaceBetween direction="horizontal" size="xs">
              <Button variant="link" onClick={() => setMoveOpen(false)} disabled={moving}>
                Cancel
              </Button>
              <Button variant="primary" onClick={() => void doMove()} disabled={!!moveValidation || dirty} loading={moving}>
                Move
              </Button>
            </SpaceBetween>
          </Box>
        }
      >
        <form
          onSubmit={(e) => {
            e.preventDefault()
            if (!dirty) void doMove()
          }}
        >
          <SpaceBetween size="m">
            {dirty && (
              <Alert type="warning">You have unsaved changes. Save them before moving the note.</Alert>
            )}
            <FormField
              label="New path"
              description="Folders are created implicitly. Must end in .md."
              errorText={moveError ?? (moveTarget !== path ? (moveValidation ?? undefined) : undefined)}
            >
              <Input
                value={moveTarget}
                onChange={({ detail }) => {
                  setMoveTarget(detail.value)
                  setMoveError(null)
                }}
                disabled={moving}
                autoFocus
                ariaLabel="New path"
              />
            </FormField>
          </SpaceBetween>
        </form>
      </Modal>

      <Modal
        visible={deleteOpen}
        onDismiss={() => !deleting && setDeleteOpen(false)}
        header="Delete note?"
        footer={
          <Box float="right">
            <SpaceBetween direction="horizontal" size="xs">
              <Button variant="link" onClick={() => setDeleteOpen(false)} disabled={deleting}>
                Cancel
              </Button>
              <Button variant="primary" onClick={() => void doDelete()} loading={deleting}>
                Delete
              </Button>
            </SpaceBetween>
          </Box>
        }
      >
        <SpaceBetween size="s">
          <Box>
            Delete <b>{meta.title}</b> (<code>{meta.path}</code>)? It disappears from your notes; previous versions are
            kept in storage history.
          </Box>
          {dirty && <Alert type="warning">Your unsaved changes will be discarded too.</Alert>}
        </SpaceBetween>
      </Modal>
    </ContentLayout>
  )
}

// ---------------------------------------------------------------------------
// Route entry
// ---------------------------------------------------------------------------

/** Note path from the (percent-encoded) location, or an error message. */
function pathFromLocation(pathname: string): { path: string } | { error: string } {
  const raw = pathname.replace(/^\/notes\//, '')
  let path: string
  try {
    path = decodeNotePath(raw)
  } catch {
    return { error: 'The URL contains a malformed percent-encoding.' }
  }
  const invalid = validateNotePath(path)
  return invalid ? { error: invalid } : { path }
}

function NoteFromLocation() {
  const { pathname } = useLocation()
  const result = pathFromLocation(pathname)
  if ('error' in result) {
    return (
      <ContentLayout header={<Header variant="h1">Invalid note path</Header>}>
        <Alert type="error" header="This is not a valid note path">
          {result.error}
        </Alert>
      </ContentLayout>
    )
  }
  // Keyed by path so moving/renaming (or following a link) starts fresh.
  return <ExistingNoteEditor key={result.path} path={result.path} />
}

export default function NoteEditorPage({ mode }: { mode: 'new' | 'edit' }) {
  return mode === 'new' ? <NewNoteEditor /> : <NoteFromLocation />
}
