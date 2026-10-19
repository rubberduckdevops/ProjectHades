import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

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
          if (id.includes('aws-amplify') || id.includes('@aws-amplify') || id.includes('@aws-crypto') || id.includes('@aws-sdk') || id.includes('@smithy')) return 'amplify'
          return 'vendor'
        },
      },
    },
  },
})
