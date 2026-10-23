import { useEffect, useState } from 'react'
import Alert from '@cloudscape-design/components/alert'
import Box from '@cloudscape-design/components/box'
import Button from '@cloudscape-design/components/button'
import Header from '@cloudscape-design/components/header'
import Table from '@cloudscape-design/components/table'
import { deleteProject, listProjects, type Project } from '../api/projects'
import { formatDateTime } from '../lib/dates'
import { saveCachedProjects } from '../lib/projects'
import { useSettings } from '../settings-store'

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

export default function ProjectsSection() {
  const { timeZone } = useSettings()
  const [projects, setProjects] = useState<Project[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    listProjects()
      .then((result) => {
        if (!cancelled) setProjects(result)
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

  const remove = async (project: Project) => {
    setBusy(project.name)
    setError(null)
    try {
      await deleteProject(project.name)
      const next = projects.filter((p) => p.name !== project.name)
      setProjects(next)
      saveCachedProjects(next.map((p) => p.name))
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setBusy(null)
    }
  }

  return (
    <Table
      variant="container"
      items={projects}
      loading={loading}
      loadingText="Loading projects"
      trackBy="name"
      header={
        <Header
          variant="h2"
          counter={loading ? undefined : `(${projects.length})`}
          description="Suggested in the journal and todos. Removing a name only drops the suggestion; entries keep it, and using it again brings it back."
        >
          Projects
        </Header>
      }
      columnDefinitions={[
        { id: 'name', header: 'Name', cell: (p) => p.name },
        { id: 'used', header: 'Last used', cell: (p) => formatDateTime(p.lastUsedAt, timeZone), width: 220 },
        {
          id: 'actions',
          header: '',
          width: 64,
          cell: (p) => (
            <Button
              variant="icon"
              iconName="remove"
              disabled={busy !== null}
              loading={busy === p.name}
              onClick={() => void remove(p)}
              ariaLabel={`Remove "${p.name}"`}
            />
          ),
        },
      ]}
      empty={
        <Box textAlign="center" color="inherit" padding="s">
          No projects yet
        </Box>
      }
      footer={error ? <Alert type="error">{error}</Alert> : undefined}
    />
  )
}
