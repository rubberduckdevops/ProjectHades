import { Authenticator, ThemeProvider } from '@aws-amplify/ui-react'
import AppLayout from '@cloudscape-design/components/app-layout'
import SideNavigation from '@cloudscape-design/components/side-navigation'
import TopNavigation from '@cloudscape-design/components/top-navigation'
import { BrowserRouter, Navigate, Route, Routes, useLocation, useNavigate } from 'react-router-dom'
import { applications } from './applications'
import JournalPage from './pages/JournalPage'
import SettingsPage from './pages/SettingsPage'
import { SettingsProvider } from './settings-context'
import { useSettings } from './settings-store'

interface ShellProps {
  userLabel: string
  onSignOut: () => void
}

function Shell({ userLabel, onSignOut }: ShellProps) {
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
            activeHref={location.pathname}
            items={applications.map((app) => ({ type: 'link' as const, text: app.text, href: app.href }))}
            onFollow={(e) => {
              if (!e.detail.external) {
                e.preventDefault()
                navigate(e.detail.href)
              }
            }}
          />
        }
        content={
          <Routes>
            <Route path="/" element={<Navigate to="/journal" replace />} />
            <Route path="/journal" element={<JournalPage />} />
            <Route path="/settings" element={<SettingsPage />} />
            <Route path="*" element={<Navigate to="/journal" replace />} />
          </Routes>
        }
      />
    </>
  )
}

function ThemedApp() {
  const { resolvedTheme } = useSettings()
  return (
    <ThemeProvider colorMode={resolvedTheme}>
      <Authenticator hideSignUp loginMechanisms={['email']}>
        {({ signOut, user }) => (
          <BrowserRouter>
            <Shell
              userLabel={user?.signInDetails?.loginId ?? user?.username ?? 'User'}
              onSignOut={() => signOut?.()}
            />
          </BrowserRouter>
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
