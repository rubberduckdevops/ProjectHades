import { createContext, useContext, type CSSProperties, type ReactNode } from 'react'
import Box from '@cloudscape-design/components/box'
import Link from '@cloudscape-design/components/link'
import TextContent from '@cloudscape-design/components/text-content'
import {
  borderRadiusContainer,
  colorBackgroundCellShaded,
  colorBackgroundContainerHeader,
  colorBorderDividerDefault,
  colorTextBodySecondary,
  fontFamilyMonospace,
  spaceScaledS,
  spaceScaledXs,
  spaceScaledXxs,
} from '@cloudscape-design/design-tokens'
import Markdown, { type Components } from 'react-markdown'
import remarkGfm from 'remark-gfm'

const cell: CSSProperties = {
  border: `1px solid ${colorBorderDividerDefault}`,
  padding: `${spaceScaledXxs} ${spaceScaledS}`,
  textAlign: 'start',
  verticalAlign: 'top',
}

const codeBlock: CSSProperties = {
  background: colorBackgroundCellShaded,
  border: `1px solid ${colorBorderDividerDefault}`,
  borderRadius: borderRadiusContainer,
  padding: spaceScaledS,
  overflowX: 'auto',
  fontFamily: fontFamilyMonospace,
  whiteSpace: 'pre',
}

const inlineCode: CSSProperties = {
  background: colorBackgroundCellShaded,
  borderRadius: '4px',
  padding: '0 4px',
  fontFamily: fontFamilyMonospace,
}

const InCodeBlock = createContext(false)

function Code({ children, className }: { children?: ReactNode; className?: string }) {
  const block = useContext(InCodeBlock)
  return (
    <code className={className} style={block ? { fontFamily: fontFamilyMonospace } : inlineCode}>
      {children}
    </code>
  )
}

const components: Components = {
  // http(s) links open in a new tab; react-markdown's default urlTransform drops unsafe schemes.
  a: ({ href, children }) => (
    <Link href={href} external={!!href && /^https?:/i.test(href)}>
      {children}
    </Link>
  ),
  table: ({ children }) => (
    <div style={{ overflowX: 'auto' }}>
      <table style={{ borderCollapse: 'collapse', margin: `${spaceScaledXs} 0` }}>{children}</table>
    </div>
  ),
  th: ({ children, style }) => (
    <th style={{ ...cell, ...style, background: colorBackgroundContainerHeader, fontWeight: 'bold' }}>
      {children}
    </th>
  ),
  td: ({ children, style }) => <td style={{ ...cell, ...style }}>{children}</td>,
  pre: ({ children }) => (
    <pre style={codeBlock}>
      <InCodeBlock.Provider value>{children}</InCodeBlock.Provider>
    </pre>
  ),
  code: ({ children, className }) => <Code className={className}>{children}</Code>,
  blockquote: ({ children }) => (
    <blockquote
      style={{
        margin: `${spaceScaledXs} 0`,
        paddingInlineStart: spaceScaledS,
        borderInlineStart: `4px solid ${colorBorderDividerDefault}`,
        color: colorTextBodySecondary,
      }}
    >
      {children}
    </blockquote>
  ),
  img: ({ src, alt }) => <img src={typeof src === 'string' ? src : undefined} alt={alt ?? ''} style={{ maxWidth: '100%' }} />,
}

/** GitHub-flavored Markdown preview. Raw HTML is shown escaped as text, never rendered (no rehype-raw). */
export default function MarkdownPreview({ content }: { content: string }) {
  if (!content.trim()) {
    return <Box color="text-status-inactive">Nothing to preview</Box>
  }
  return (
    <TextContent>
      <Markdown remarkPlugins={[remarkGfm]} components={components}>
        {content}
      </Markdown>
    </TextContent>
  )
}
