import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],
  server: {
    proxy: {
      // native `just dev-api` runs on 8081; the embedded dashboard and the
      // dev server share the same-origin /api/rpc surface
      '/api': 'http://localhost:8081',
    },
  },
})
