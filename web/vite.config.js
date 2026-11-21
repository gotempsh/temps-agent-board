import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vite'
import tailwind from '@tailwindcss/vite'

const consoleWeb = fileURLToPath(new URL('../vendor/temps/web/', import.meta.url))
export default defineConfig({
  plugins: [tailwind()],
  esbuild: { jsx: 'automatic' },
  resolve: {
    alias: [
      { find: '@', replacement: consoleWeb + 'src' },
      ...['@temps-sdk/ds', '@temps-sdk/ui', 'react-dom', 'react', 'lucide-react'].map(name => ({
        find: name, replacement: consoleWeb + 'node_modules/' + name,
      })),
    ],
  },
  build: {
    rollupOptions: {
      input: 'main.jsx',
      output: { entryFileNames: 'board.js', assetFileNames: asset => asset.names.some(name => name.endsWith('.css')) ? 'board.css' : '[name][extname]' },
    },
  },
})
