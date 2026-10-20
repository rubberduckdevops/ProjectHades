import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// react-markdown + remark-gfm and the unified/micromark ecosystem they pull in.
// Kept out of `vendor` so it only loads when a note preview is opened (lazy import).
const MARKDOWN_PACKAGES =
  /node_modules\/(react-markdown|remark-[^/]+|rehype-[^/]+|micromark[^/]*|mdast-[^/]+|hast-[^/]+|unist-[^/]+|unified|vfile[^/]*|bail|trough|is-plain-obj|devlop|property-information|space-separated-tokens|comma-separated-tokens|decode-named-character-reference|character-entities[^/]*|character-reference-invalid|is-alphanumerical|is-alphabetical|is-decimal|is-hexadecimal|ccount|escape-string-regexp|markdown-table|longest-streak|zwitch|trim-lines|html-url-attributes|style-to-js|style-to-object|inline-style-parser|estree-util-[^/]+|extend|@ungap\/structured-clone)\//

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  server: { port: 5173 },
  build: {
    chunkSizeWarningLimit: 1500,
    rollupOptions: {
      output: {
        manualChunks(id: string) {
          if (!id.includes('node_modules')) return undefined
          if (id.includes('@cloudscape-design')) return 'cloudscape'
          if (MARKDOWN_PACKAGES.test(id)) return 'markdown'
          if (id.includes('aws-amplify') || id.includes('@aws-amplify') || id.includes('@aws-crypto') || id.includes('@aws-sdk') || id.includes('@smithy')) return 'amplify'
          return 'vendor'
        },
      },
    },
  },
})
