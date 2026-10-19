import { useMemo, useState } from 'react'
import Box from '@cloudscape-design/components/box'
import Container from '@cloudscape-design/components/container'
import ContentLayout from '@cloudscape-design/components/content-layout'
import FormField from '@cloudscape-design/components/form-field'
import Header from '@cloudscape-design/components/header'
import RadioGroup from '@cloudscape-design/components/radio-group'
import Select, { type SelectProps } from '@cloudscape-design/components/select'
import SpaceBetween from '@cloudscape-design/components/space-between'
import ApiTokenSection from '../components/ApiTokenSection'
import { browserTimeZone, formatTime, supportedTimeZones } from '../lib/dates'
import type { ThemePreference } from '../lib/settings'
import { useSettings } from '../settings-store'

const BROWSER_DEFAULT = '__browser__'

export default function SettingsPage() {
  const { settings, updateSettings, timeZone } = useSettings()
  const [nowIso] = useState(() => new Date().toISOString())

  const tzOptions = useMemo<SelectProps.Options>(() => {
    const browser = browserTimeZone()
    return [
      { value: BROWSER_DEFAULT, label: 'Browser default', description: browser },
      ...supportedTimeZones().map((tz) => ({ value: tz, label: tz })),
    ]
  }, [])

  const selectedTz =
    tzOptions.find((o) => 'value' in o && o.value === (settings.timeZone ?? BROWSER_DEFAULT)) ?? null

  return (
    <ContentLayout
      header={
        <Header variant="h1" description="Appearance and time settings are saved in this browser.">
          Profile settings
        </Header>
      }
    >
      <SpaceBetween size="l">
        <Container header={<Header variant="h2">Appearance</Header>}>
          <FormField label="Theme">
            <RadioGroup
              value={settings.theme}
              onChange={({ detail }) => updateSettings({ theme: detail.value as ThemePreference })}
              items={[
                { value: 'system', label: 'System', description: 'Follow your operating system setting' },
                { value: 'light', label: 'Light' },
                { value: 'dark', label: 'Dark' },
              ]}
            />
          </FormField>
        </Container>
        <Container header={<Header variant="h2">Time</Header>}>
          <FormField
            label="Timezone"
            description={'Used for the journal\'s "today" and for displayed times.'}
            constraintText={
              <Box variant="small">
                Current time in {timeZone}: {formatTime(nowIso, timeZone)}
              </Box>
            }
          >
            <Select
              selectedOption={selectedTz}
              onChange={({ detail }) => {
                const value = detail.selectedOption.value
                updateSettings({ timeZone: !value || value === BROWSER_DEFAULT ? null : value })
              }}
              options={tzOptions}
              filteringType="auto"
              filteringPlaceholder="Search timezones"
              virtualScroll
            />
          </FormField>
        </Container>
        <ApiTokenSection />
      </SpaceBetween>
    </ContentLayout>
  )
}
