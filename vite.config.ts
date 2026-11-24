/// <reference types="vitest/config" />

import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  test: { environment: 'node', include: ['src/**/*.test.ts'] },
  server: {
    host: '127.0.0.1',
    port: 1432,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
})
