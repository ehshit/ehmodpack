import { statSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import vuetify from 'vite-plugin-vuetify'

const privacyFile = fileURLToPath(new URL('./src/locales/en-US/ui.json', import.meta.url))
const privacyUpdated = new Intl.DateTimeFormat('en-GB', {
  day: '2-digit',
  month: 'short',
  year: 'numeric'
}).format(statSync(privacyFile).mtime)

export default defineConfig({
  define: {
    __PRIVACY_UPDATED_AT__: JSON.stringify(privacyUpdated)
  },
  plugins: [vue(), vuetify()],
  build: {
    outDir: 'dist',
    emptyOutDir: true
  }
})
