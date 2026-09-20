import { defineConfig } from 'vite'
import type { ViteDevServer } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { readFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'

const STATE_DIR = resolve(__dirname, '../.dual-brain/state')
const FALLBACK_DIR = resolve(__dirname, '../.dualbrain')

const STATE_FILES = ['providers', 'routing', 'rooms', 'decisions', 'outcomes', 'costs'] as const

async function readJson(name: string) {
  for (const dir of [STATE_DIR, FALLBACK_DIR]) {
    try {
      const raw = await readFile(join(dir, `${name}.json`), 'utf-8')
      return JSON.parse(raw)
    } catch { /* try next */ }
  }
  return null
}

function stateApiPlugin() {
  return {
    name: 'cortex-state-api',
    configureServer(server: ViteDevServer) {
      server.middlewares.use(async (req, res, next) => {
        if (req.url === '/api/cortex/state') {
          const [providers, routing, rooms, decisions, outcomes, costs] = await Promise.all(
            STATE_FILES.map(readJson)
          )
          res.setHeader('Content-Type', 'application/json')
          res.end(JSON.stringify({
            providers,
            routing,
            rooms,
            decisions: decisions ?? [],
            outcomes: outcomes ?? [],
            costs,
          }))
          return
        }
        next()
      })
    },
  }
}

export default defineConfig({
  plugins: [stateApiPlugin(), react(), tailwindcss()],
  build: {
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes('node_modules')) return undefined;
          if (id.includes('@clerk')) return 'auth-vendor';
          if (id.includes('lucide-react')) return 'icons-vendor';
          return 'vendor';
        },
      },
    },
  },
  server: {
    host: '0.0.0.0',
    port: 5001,
    allowedHosts: true,
    proxy: {
      '/api': {
        target: 'http://localhost:3001',
        changeOrigin: true,
      },
    },
  },
})
