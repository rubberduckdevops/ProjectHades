import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import Autosuggest from '@cloudscape-design/components/autosuggest'
import Box from '@cloudscape-design/components/box'
import Button from '@cloudscape-design/components/button'
import Checkbox from '@cloudscape-design/components/checkbox'
import ContentLayout from '@cloudscape-design/components/content-layout'
import DatePicker from '@cloudscape-design/components/date-picker'
import ExpandableSection from '@cloudscape-design/components/expandable-section'
import Flashbar, { type FlashbarProps } from '@cloudscape-design/components/flashbar'
import FormField from '@cloudscape-design/components/form-field'
import Grid from '@cloudscape-design/components/grid'
import Header from '@cloudscape-design/components/header'
import Input, { type InputProps } from '@cloudscape-design/components/input'
import Select, { type SelectProps } from '@cloudscape-design/components/select'
import SpaceBetween from '@cloudscape-design/components/space-between'
import StatusIndicator from '@cloudscape-design/components/status-indicator'
import Table, { type TableProps } from '@cloudscape-design/components/table'
import { createTodo, deleteTodo, listTodos, updateTodo, type Todo, type TodoPatch } from '../api/todos'
import { calendarDate, formatDateTime } from '../lib/dates'
import { absorbProjects, loadRecentProjects, rememberProject } from '../lib/projects'
import { dueStatus, formatDue, sortDone, sortOpen, upsertTodo } from '../lib/todos'
import { useSettings } from '../settings-store'

const ALL_PROJECTS: SelectProps.Option = { label: 'All projects', value: '' }
const NO_PROJECT: SelectProps.Option = { label: 'No project', value: '\u0000none' }

const DATE_PICKER_LABELS = {
  openCalendarAriaLabel: (selected: string | null) => (selected ? `Change due date, ${selected}` : 'Choose due date'),
  placeholder: 'YYYY/MM/DD',
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

function projectsIn(todos: Todo[]): string[] {
  return todos.flatMap((t) => (t.project ? [t.project] : []))
}

function DueCell({ due, today }: { due: string | null; today: string }) {
  if (!due) return <Box color="text-status-inactive">—</Box>
  const label = formatDue(due, today)
  switch (dueStatus(due, today)) {
    case 'overdue':
      return <StatusIndicator type="error">{label}</StatusIndicator>
    case 'today':
      return <StatusIndicator type="warning">{label}</StatusIndicator>
    default:
      return <>{label}</>
  }
}

export default function TodosPage() {
  const { timeZone } = useSettings()
  const [todos, setTodos] = useState<Todo[]>([])
  const [loading, setLoading] = useState(true)
  const [busyIds, setBusyIds] = useState<ReadonlySet<string>>(new Set())
  const [flash, setFlash] = useState<FlashbarProps.MessageDefinition[]>([])
  const flashId = useRef(0)

  const [title, setTitle] = useState('')
  const [project, setProject] = useState('')
  const [due, setDue] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const titleRef = useRef<InputProps.Ref>(null)
  const [recentProjects, setRecentProjects] = useState(loadRecentProjects)
  const [projectFilter, setProjectFilter] = useState<SelectProps.Option>(ALL_PROJECTS)

  const [mountedAt] = useState(() => new Date())
  const today = calendarDate(mountedAt, timeZone)

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

  const load = useCallback(
    (isCancelled: () => boolean = () => false) =>
      listTodos()
        .then((result) => {
          if (isCancelled()) return
          setTodos(result)
          setRecentProjects(absorbProjects(projectsIn(result)))
        })
        .catch((err: unknown) => {
          if (!isCancelled()) pushError('Failed to load todos', err)
        })
        .finally(() => {
          if (!isCancelled()) setLoading(false)
        }),
    [pushError],
  )

  useEffect(() => {
    let cancelled = false
    void load(() => cancelled)
    return () => {
      cancelled = true
    }
  }, [load])

  const refresh = () => {
    setLoading(true)
    void load()
  }

  const setBusy = (id: string, busy: boolean) =>
    setBusyIds((prev) => {
      const next = new Set(prev)
      if (busy) next.add(id)
      else next.delete(id)
      return next
    })

  /** PATCH a todo; resolves to false (after showing the error) on failure. */
  const patch = async (todo: Todo, changes: TodoPatch, failure: string): Promise<boolean> => {
    setBusy(todo.id, true)
    try {
      const updated = await updateTodo(todo.id, changes)
      setTodos((prev) => upsertTodo(prev, updated))
      if (updated.project) setRecentProjects(rememberProject(updated.project))
      return true
    } catch (err) {
      pushError(failure, err)
      return false
    } finally {
      setBusy(todo.id, false)
    }
  }

  const remove = async (todo: Todo) => {
    setBusy(todo.id, true)
    try {
      await deleteTodo(todo.id)
      setTodos((prev) => prev.filter((t) => t.id !== todo.id))
    } catch (err) {
      pushError('Failed to delete todo', err)
    } finally {
      setBusy(todo.id, false)
    }
  }

  const trimmedTitle = title.trim()
  const canAdd = trimmedTitle.length > 0 && !submitting

  const add = async () => {
    if (!canAdd) return
    setSubmitting(true)
    try {
      const projectName = project.trim()
      const todo = await createTodo({
        title: trimmedTitle,
        project: projectName || undefined,
        due: due || undefined,
      })
      setTodos((prev) => upsertTodo(prev, todo))
      if (todo.project) setRecentProjects(rememberProject(todo.project))
      setTitle('')
      setDue('')
      // Keep the project: adding several tasks for one project in a row is common.
      titleRef.current?.focus()
    } catch (err) {
      pushError('Failed to add todo', err)
    } finally {
      setSubmitting(false)
    }
  }

  const matchesFilter = useCallback(
    (t: Todo) =>
      projectFilter.value === ''
        ? true
        : projectFilter.value === NO_PROJECT.value
          ? t.project === null
          : t.project === projectFilter.value,
    [projectFilter],
  )

  const open = useMemo(() => sortOpen(todos).filter(matchesFilter), [todos, matchesFilter])
  const done = useMemo(() => sortDone(todos).filter(matchesFilter), [todos, matchesFilter])
  const allOpen = todos.filter((t) => !t.done)
  const overdueCount = allOpen.filter((t) => dueStatus(t.due, today) === 'overdue').length
  const summary =
    allOpen.length === 0
      ? 'All clear.'
      : `${allOpen.length} open${overdueCount > 0 ? ` · ${overdueCount} overdue` : ''}`

  const projectOptions = useMemo(() => recentProjects.map((value) => ({ value })), [recentProjects])
  const filterOptions = useMemo<SelectProps.Options>(() => {
    const names = [...new Set(projectsIn(todos))].sort((a, b) => a.localeCompare(b))
    return [ALL_PROJECTS, NO_PROJECT, ...names.map((name) => ({ label: name, value: name }))]
  }, [todos])

  const doneColumn: TableProps.ColumnDefinition<Todo> = {
    id: 'done',
    header: '',
    width: 56,
    cell: (item) => (
      <Checkbox
        checked={item.done}
        disabled={busyIds.has(item.id)}
        onChange={({ detail }) =>
          void patch(item, { done: detail.checked }, detail.checked ? 'Failed to complete todo' : 'Failed to reopen todo')
        }
        ariaLabel={item.done ? `Reopen "${item.title}"` : `Complete "${item.title}"`}
      />
    ),
  }

  const deleteColumn: TableProps.ColumnDefinition<Todo> = {
    id: 'actions',
    header: '',
    width: 64,
    cell: (item) => (
      <Button
        variant="icon"
        iconName="remove"
        disabled={busyIds.has(item.id)}
        onClick={() => void remove(item)}
        ariaLabel={`Delete "${item.title}"`}
      />
    ),
  }

  const projectCell = (item: Todo) => item.project ?? <Box color="text-status-inactive">—</Box>

  const openColumns: TableProps.ColumnDefinition<Todo>[] = [
    doneColumn,
    {
      id: 'title',
      header: 'Task',
      cell: (item) => item.title,
      editConfig: {
        ariaLabel: 'Task',
        validation: (_item, value) => (typeof value === 'string' && !value.trim() ? 'Task can’t be empty' : undefined),
        editingCell: (item, { currentValue, setValue }) => (
          <Input
            autoFocus
            value={currentValue ?? item.title}
            onChange={({ detail }) => setValue(detail.value)}
            ariaLabel="Task"
          />
        ),
      },
    },
    {
      id: 'project',
      header: 'Project',
      width: 180,
      cell: projectCell,
      editConfig: {
        ariaLabel: 'Project',
        editingCell: (item, { currentValue, setValue }) => (
          <Autosuggest
            value={currentValue ?? item.project ?? ''}
            onChange={({ detail }) => setValue(detail.value)}
            options={projectOptions}
            enteredTextLabel={(value) => `Use "${value}"`}
            placeholder="None"
            ariaLabel="Project"
          />
        ),
      },
    },
    {
      id: 'due',
      header: 'Due',
      width: 190,
      cell: (item) => <DueCell due={item.due} today={today} />,
      editConfig: {
        ariaLabel: 'Due date',
        editingCell: (item, { currentValue, setValue }) => (
          <DatePicker
            value={currentValue ?? item.due ?? ''}
            onChange={({ detail }) => setValue(detail.value)}
            {...DATE_PICKER_LABELS}
          />
        ),
      },
    },
    deleteColumn,
  ]

  const doneColumns: TableProps.ColumnDefinition<Todo>[] = [
    doneColumn,
    {
      id: 'title',
      header: 'Task',
      cell: (item) => (
        <Box color="text-status-inactive">
          <s>{item.title}</s>
        </Box>
      ),
    },
    { id: 'project', header: 'Project', width: 180, cell: projectCell },
    {
      id: 'completed',
      header: 'Completed',
      width: 210,
      cell: (item) => (item.completedAt ? formatDateTime(item.completedAt, timeZone) : '—'),
    },
    deleteColumn,
  ]

  const submitEdit: TableProps.SubmitEditFunction<Todo> = async (item, column, value) => {
    const text = typeof value === 'string' ? value : ''
    let changes: TodoPatch
    if (column.id === 'title') changes = { title: text.trim() }
    else if (column.id === 'project') changes = { project: text.trim() || null }
    else if (column.id === 'due') changes = { due: text || null }
    else return
    // Throwing keeps the cell in edit mode so the change isn't silently lost.
    if (!(await patch(item, changes, 'Failed to update todo'))) throw new Error('update failed')
  }

  return (
    <ContentLayout header={<Header variant="h1" description={summary}>Todos</Header>}>
      <SpaceBetween size="l">
        {flash.length > 0 && <Flashbar items={flash} />}

        {/* Enter in the task field adds; the form itself never submits. */}
        <form onSubmit={(e) => e.preventDefault()}>
          <Grid
            gridDefinition={[
              { colspan: { default: 12, m: 6 } },
              { colspan: { default: 12, xs: 5, m: 2 } },
              { colspan: { default: 12, xs: 5, m: 3 } },
              { colspan: { default: 12, xs: 2, m: 1 } },
            ]}
          >
            <FormField label="New task" stretch>
              <Input
                ref={titleRef}
                value={title}
                onChange={({ detail }) => setTitle(detail.value)}
                onKeyDown={({ detail }) => {
                  if (detail.key === 'Enter') void add()
                }}
                placeholder="What needs doing?"
                disabled={submitting}
                ariaLabel="New task"
              />
            </FormField>
            <FormField label="Project" stretch>
              <Autosuggest
                value={project}
                onChange={({ detail }) => setProject(detail.value)}
                options={projectOptions}
                enteredTextLabel={(value) => `Use "${value}"`}
                placeholder="None"
                empty="No recent projects"
                disabled={submitting}
                ariaLabel="Project"
              />
            </FormField>
            <FormField label="Due" stretch>
              <DatePicker
                value={due}
                onChange={({ detail }) => setDue(detail.value)}
                disabled={submitting}
                {...DATE_PICKER_LABELS}
              />
            </FormField>
            <FormField label={<span aria-hidden="true">&nbsp;</span>} stretch>
              <Button variant="primary" formAction="none" onClick={() => void add()} disabled={!canAdd} loading={submitting}>
                Add
              </Button>
            </FormField>
          </Grid>
        </form>

        <Table
          variant="container"
          items={open}
          loading={loading}
          loadingText="Loading todos"
          trackBy="id"
          wrapLines
          columnDefinitions={openColumns}
          submitEdit={submitEdit}
          ariaLabels={{
            tableLabel: 'Open todos',
            activateEditLabel: (column, item) => `Edit ${column.header} of "${item.title}"`,
            cancelEditLabel: (column) => `Cancel editing ${column.header}`,
            submitEditLabel: (column) => `Save ${column.header}`,
          }}
          empty={
            <Box textAlign="center" color="inherit" padding="m">
              <b>{projectFilter.value === '' ? 'Nothing to do' : 'Nothing to do for this project'}</b>
            </Box>
          }
          header={
            <Header
              counter={`(${open.length})`}
              description="Click a task, project or due date to edit it."
              actions={
                <SpaceBetween direction="horizontal" size="xs">
                  <Select
                    selectedOption={projectFilter}
                    onChange={({ detail }) => setProjectFilter(detail.selectedOption)}
                    options={filterOptions}
                    ariaLabel="Filter by project"
                  />
                  <Button iconName="refresh" onClick={refresh} disabled={loading} ariaLabel="Refresh">
                    Refresh
                  </Button>
                </SpaceBetween>
              }
            >
              Open
            </Header>
          }
        />

        <ExpandableSection variant="container" headerText="Completed" headerCounter={`(${done.length})`}>
          <Table
            variant="embedded"
            items={done}
            trackBy="id"
            wrapLines
            columnDefinitions={doneColumns}
            ariaLabels={{ tableLabel: 'Completed todos' }}
            empty={
              <Box textAlign="center" color="inherit" padding="s">
                Nothing completed yet
              </Box>
            }
          />
        </ExpandableSection>
      </SpaceBetween>
    </ContentLayout>
  )
}
