import Box from '@cloudscape-design/components/box'
import Button from '@cloudscape-design/components/button'
import ColumnLayout from '@cloudscape-design/components/column-layout'
import Container from '@cloudscape-design/components/container'
import ContentLayout from '@cloudscape-design/components/content-layout'
import Header from '@cloudscape-design/components/header'
import SpaceBetween from '@cloudscape-design/components/space-between'
import { useNavigate } from 'react-router-dom'
import { applications } from '../applications'

/** One-line pitches for each app, keyed by route. Unknown apps just show their name. */
const APP_BLURBS: Record<string, string> = {
  '/journal': 'A quick, timestamped log of your day. Tag entries with a project, then copy the day out as Markdown.',
  '/notes': 'Longer-lived Markdown notes in folders, with edit history. Semantic search is on the way.',
}

const HOW_IT_WORKS: { title: string; body: string }[] = [
  {
    title: 'You sign in',
    body: 'Amazon Cognito checks who you are. Every request carries your token, and the API only ever shows you your own data.',
  },
  {
    title: 'You write',
    body: 'Journal entries go to DynamoDB, keyed by you and the moment you wrote them. Notes are Markdown files in a versioned S3 bucket.',
  },
  {
    title: 'It stays yours',
    body: 'Everything runs in your own AWS account: a Rust API on Lambda behind CloudFront. No servers idling, nothing shared.',
  },
  {
    title: 'You find it later',
    body: "Notes will be indexed by meaning, not just keywords, so “that idea about caching” finds the note even if you never wrote “caching”.",
  },
]

export default function HomePage() {
  const navigate = useNavigate()

  return (
    <ContentLayout
      header={
        <Header variant="h1" description="A personal underworld for your thoughts.">
          Welcome to Hades
        </Header>
      }
    >
      <SpaceBetween size="l">
        <Container header={<Header variant="h2">What is Hades?</Header>}>
          <Box variant="p">
            Hades is a small, private workspace for the things you'd otherwise lose: what you did today, what you
            were thinking, and the notes you swear you'll come back to. It's a handful of focused apps behind one
            sign-in, hosted entirely in your own AWS account.
          </Box>
        </Container>

        <Container header={<Header variant="h2">Applications</Header>}>
          <ColumnLayout columns={Math.min(Math.max(applications.length, 1), 3)} variant="text-grid">
            {applications.map((app) => (
              <SpaceBetween key={app.href} size="xs">
                <Box variant="h3">{app.text}</Box>
                <Box variant="p" color="text-body-secondary">
                  {APP_BLURBS[app.href] ?? ''}
                </Box>
                <Button onClick={() => navigate(app.href)} iconName="angle-right" iconAlign="right">
                  Open {app.text}
                </Button>
              </SpaceBetween>
            ))}
          </ColumnLayout>
        </Container>

        <Container header={<Header variant="h2">How it works</Header>}>
          <ColumnLayout columns={4} variant="text-grid">
            {HOW_IT_WORKS.map((step, i) => (
              <SpaceBetween key={step.title} size="xxs">
                <Box variant="awsui-key-label">
                  {i + 1}. {step.title}
                </Box>
                <Box variant="p">{step.body}</Box>
              </SpaceBetween>
            ))}
          </ColumnLayout>
        </Container>

        <Container header={<Header variant="h2">Why “Hades”?</Header>}>
          <SpaceBetween size="s">
            <Box variant="p">
              In Greek myth, everything ends up in Hades eventually, and nothing leaves without the boss's say-so.
              That's the deal here too: your thoughts go in, they stay put, and the door is guarded by a
              three-headed dog. (Fine, it's Cognito. It has fewer heads but much better audit logs.)
            </Box>
            <Box variant="p">
              Persephone ate six pomegranate seeds and was bound to the underworld for half the year. You'll write
              far more than six notes, so we made sure there's a way back out: copy anything as Markdown whenever
              you like.
            </Box>
          </SpaceBetween>
        </Container>
      </SpaceBetween>
    </ContentLayout>
  )
}
