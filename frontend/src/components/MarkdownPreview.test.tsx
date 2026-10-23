import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import MarkdownPreview from './MarkdownPreview'

const render = (content: string) => renderToStaticMarkup(<MarkdownPreview content={content} />)

describe('MarkdownPreview', () => {
  it('escapes raw HTML instead of rendering it', () => {
    const html = render('hi <b>bold</b> <script>alert(1)</script> <img src=x onerror=alert(1)>')
    expect(html).not.toContain('<script')
    expect(html).not.toContain('<b>')
    expect(html).not.toContain('<img')
    expect(html).toContain('&lt;script&gt;')
  })

  it('drops javascript: links', () => {
    expect(render('[x](javascript:alert(1))')).not.toContain('javascript:')
  })

  it('renders GFM tables, task lists and strikethrough', () => {
    const html = render('| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n\n~~gone~~')
    expect(html).toContain('<table')
    expect(html).toContain('<td')
    expect(html).toContain('type="checkbox"')
    expect(html).toContain('<del>')
  })

  it('renders fenced code blocks', () => {
    expect(render('```ts\nconst a = 1\n```')).toMatch(/<pre[^>]*><code class="language-ts"/)
  })

  it('keeps single newlines as line breaks only in compact mode', () => {
    const compact = renderToStaticMarkup(<MarkdownPreview content={'one\ntwo'} compact />)
    expect(compact).toContain('<br/>')
    expect(render('one\ntwo')).not.toContain('<br/>')
  })

  it('links bare URLs', () => {
    expect(render('see https://example.com')).toContain('href="https://example.com"')
  })
})
