import '@cloudscape-design/global-styles/index.css'
import '@aws-amplify/ui-react/styles.css'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import Alert from '@cloudscape-design/components/alert'
import Box from '@cloudscape-design/components/box'
import { loadConfig } from './config'
import { applyTheme, loadSettings } from './lib/settings'
import App from './App'

// Apply the saved theme before anything renders, to avoid a light-mode flash.
applyTheme(loadSettings().theme)

const root = createRoot(document.getElementById('root')!)

async function bootstrap() {
  try {
    await loadConfig()
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err)
    root.render(
      <StrictMode>
        <Box padding="xl">
          <Alert type="error" header="Configuration error">
            Could not load runtime configuration: {message}. Make sure <code>/config.json</code> exists
            and contains non-empty <code>apiUrl</code>, <code>region</code>, <code>userPoolId</code> and{' '}
            <code>userPoolClientId</code> values. For local development, copy{' '}
            <code>public/config.example.json</code> to <code>public/config.json</code>.
          </Alert>
        </Box>
      </StrictMode>,
    )
    return
  }
  root.render(
    <StrictMode>
      <App />
    </StrictMode>,
  )
}

void bootstrap()
