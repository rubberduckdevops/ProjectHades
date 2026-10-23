import { request } from './client'

export interface Project {
  name: string
  lastUsedAt: string
}

/** Every project the user has used, most recently used first. */
export function listProjects(): Promise<Project[]> {
  return request<Project[]>('/projects')
}

/** Forget a project name. Entries that use it keep it. */
export async function deleteProject(name: string): Promise<void> {
  await request<void>(`/projects/${encodeURIComponent(name)}`, { method: 'DELETE' })
}
