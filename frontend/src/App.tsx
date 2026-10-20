import { Authenticator, ThemeProvider } from '@aws-amplify/ui-react'
import AppLayout from '@cloudscape-design/components/app-layout'
import SideNavigation from '@cloudscape-design/components/side-navigation'
import TopNavigation from '@cloudscape-design/components/top-navigation'
import { createContext, useContext, useMemo } from 'react'
import { createBrowserRouter, Navigate, Outlet, RouterProvider, useLocation, useNavigate } from 'react-router-dom'
import { activeApplicationHref, applications } from './applications'
import HomePage from './pages/HomePage'
import JournalPage from './pages/JournalPage'
import NoteEditorPage from './pages/NoteEditorPage'
import NotesPage from './pages/NotesPage'
import SettingsPage from './pages/SettingsPage'
import { SettingsProvider } from './settings-context'
import { useSettings } from './settings-store'

interface SessionInfo {
  userLabel: string
  onSignOut: () => void
}

// The router is created once at module level (a data router is needed for
// useBlocker), so the signed-in user reaches the Shell through context.
const SessionContext = createContext<SessionInfo>({ userLabel: 'User', onSignOut: () => {} })

function Shell() {
  const { userLabel, onSignOut } = useContext(SessionContext)
  const navigate = useNavigate()
  const location = useLocation()

  return (
    <>
      <div id="top-nav" style={{ position: 'sticky', top: 0, zIndex: 1002 }}>
        <TopNavigation
          identity={{
            href: '/',
            title: 'Hades',
            onFollow: (e) => {
              e.preventDefault()
              navigate('/')
            },
          }}
          utilities={[
            {
              type: 'menu-dropdown',
              text: userLabel,
              iconName: 'user-profile',
              items: [
                { id: 'settings', text: 'Profile settings', iconName: 'settings' },
                { id: 'signout', text: 'Sign out' },
              ],
              onItemClick: (e) => {
                if (e.detail.id === 'settings') navigate('/settings')
                if (e.detail.id === 'signout') onSignOut()
              },
            },
          ]}
        />
      </div>
      <AppLayout
        headerSelector="#top-nav"
        toolsHide
        navigation={
          <SideNavigation
            header={{ text: 'Applications', href: '/' }}
            activeHref={activeApplicationHref(location.pathname)}
            items={applications.map((app) => ({ type: 'link' as const, text: app.text, href: app.href }))}
            onFollow={(e) => {
              if (!e.detail.external) {
                e.preventDefault()
                navigate(e.detail.href)
              }
            }}
          />
        }
        content={<Outlet />}
      />
    </>
  )
}

const router = createBrowserRouter([
  {
    element: <Shell />,
    children: [
      { path: '/', element: <HomePage /> },
      { path: '/journal', element: <JournalPage /> },
      { path: '/notes', element: <NotesPage /> },
      { path: '/notes/new', element: <NoteEditorPage mode="new" /> },
      { path: '/notes/*', element: <NoteEditorPage mode="edit" /> },
      { path: '/settings', element: <SettingsPage /> },
      { path: '*', element: <Navigate to="/" replace /> },
    ],
  },
])

function AuthenticatedApp({ userLabel, onSignOut }: SessionInfo) {
  const session = useMemo(() => ({ userLabel, onSignOut }), [userLabel, onSignOut])
  return (
    <SessionContext.Provider value={session}>
      <RouterProvider router={router} />
    </SessionContext.Provider>
  )
}

function ThemedApp() {
  const { resolvedTheme } = useSettings()
  return (
    <ThemeProvider colorMode={resolvedTheme}>
      <Authenticator hideSignUp loginMechanisms={['email']}>
        {({ signOut, user }) => (
          <AuthenticatedApp
            userLabel={user?.signInDetails?.loginId ?? user?.username ?? 'User'}
            onSignOut={() => signOut?.()}
          />
        )}
      </Authenticator>
    </ThemeProvider>
  )
}

export default function App() {
  return (
    <SettingsProvider>
      <ThemedApp />
    </SettingsProvider>
  )
}
