import { useCallback, useEffect, useState } from 'react'
import Alert from '@cloudscape-design/components/alert'
import Box from '@cloudscape-design/components/box'
import Button from '@cloudscape-design/components/button'
import Container from '@cloudscape-design/components/container'
import CopyToClipboard from '@cloudscape-design/components/copy-to-clipboard'
import Header from '@cloudscape-design/components/header'
import KeyValuePairs from '@cloudscape-design/components/key-value-pairs'
import SpaceBetween from '@cloudscape-design/components/space-between'
import StatusIndicator from '@cloudscape-design/components/status-indicator'
import { getAccessTokenInfo, type AccessTokenInfo } from '../api/client'
import { getConfig } from '../config'
import { absoluteApiUrl, curlExample, formatRemaining } from '../lib/apiToken'
import { formatTime } from '../lib/dates'
import { useSettings } from '../settings-store'

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

export default function ApiTokenSection() {
  const { timeZone } = useSettings()
  const [info, setInfo] = useState<AccessTokenInfo | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [refreshing, setRefreshing] = useState(false)
  const [now, setNow] = useState(() => Date.now())

  const apiBase = absoluteApiUrl(getConfig().apiUrl, window.location.origin)

  useEffect(() => {
    let cancelled = false
    getAccessTokenInfo()
      .then((result) => {
        if (!cancelled) setInfo(result)
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(errorMessage(err))
      })
    const timer = setInterval(() => setNow(Date.now()), 15_000)
    return () => {
      cancelled = true
      clearInterval(timer)
    }
  }, [])

  const refresh = useCallback(async () => {
    setRefreshing(true)
    setError(null)
    try {
      setInfo(await getAccessTokenInfo(true))
      setNow(Date.now())
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setRefreshing(false)
    }
  }, [])

  const expired = info !== null && info.expiresAt <= now

  return (
    <Container
      header={
        <Header
          variant="h2"
          description="Use your current access token to call the Hades API from scripts or the command line."
          actions={
            <Button iconName="refresh" loading={refreshing} onClick={() => void refresh()}>
              Get fresh token
            </Button>
          }
        >
          API token
        </Header>
      }
    >
      <SpaceBetween size="l">
        <Alert type="warning" header="Treat this token like a password">
          Anyone who has it can read and change your journal until it expires. Don&apos;t paste it into chats,
          tickets, screenshots or shared files, and don&apos;t commit it to source control.
        </Alert>

        {error && (
          <Alert type="error" header="Couldn't get a token">
            {error}
          </Alert>
        )}

        <KeyValuePairs
          columns={2}
          items={[
            {
              label: 'Access token',
              value: info ? (
                <CopyToClipboard
                  variant="button"
                  textToCopy={info.token}
                  copyButtonText="Copy token"
                  copySuccessText="Token copied"
                  copyErrorText="Couldn't copy the token"
                  disabled={expired}
                />
              ) : error ? (
                <StatusIndicator type="error">Unavailable</StatusIndicator>
              ) : (
                <StatusIndicator type="loading">Loading</StatusIndicator>
              ),
            },
            {
              label: 'Expires',
              value: info ? (
                expired ? (
                  <StatusIndicator type="warning">Expired — get a fresh token</StatusIndicator>
                ) : (
                  <StatusIndicator type="success">
                    {formatTime(new Date(info.expiresAt).toISOString(), timeZone)} ({timeZone}),{' '}
                    {formatRemaining(info.expiresAt, now)}
                  </StatusIndicator>
                )
              ) : (
                '-'
              ),
            },
          ]}
        />

        <SpaceBetween size="xs">
          <Box variant="awsui-key-label">Example</Box>
          <Box variant="p" color="text-body-secondary">
            Paste the token into the <code>HADES_TOKEN</code> variable, then:
          </Box>
          <Box variant="pre">
            <code>{curlExample(apiBase)}</code>
          </Box>
          <CopyToClipboard
            variant="inline"
            textToCopy={curlExample(apiBase)}
            copyButtonText="Copy example"
            copySuccessText="Example copied"
            copyErrorText="Couldn't copy the example"
            textToDisplay="Copy example"
          />
        </SpaceBetween>
      </SpaceBetween>
    </Container>
  )
}
