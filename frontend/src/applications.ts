export interface Application {
  text: string
  href: string
}

export const applications: Application[] = [
  { text: 'Journal', href: '/journal' },
  { text: 'Notes', href: '/notes' },
]

/** The application href to highlight for `pathname` (matches the href and anything below it). */
export function activeApplicationHref(pathname: string): string {
  const app = applications.find((a) => pathname === a.href || pathname.startsWith(`${a.href}/`))
  return app?.href ?? pathname
}
