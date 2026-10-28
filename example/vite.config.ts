import { writeFileSync } from 'node:fs'
import { defineConfig } from 'vite'
import tailwindcss from '@tailwindcss/vite'
import { inventory, description, main as coverage } from '../tests/checks/coverage.mjs'

const evidence = coverage([])
type ArchivedCell = { status: string; reason: string; evidence: { status: string; assertion: string }[] }
const cells = evidence.cells as Map<string, Record<string, ArchivedCell>>
const members = inventory.map(member => ({
  id: member.id,
  scope: member.scope,
  purpose: 'purpose' in member ? member.purpose : description(...member.id.split('::')),
  na: member.na,
  archived: Object.fromEntries(Object.entries(cells.get(member.id)!).map(([platform, cell]) => [platform, {
    status: cell.status,
    reason: cell.reason,
    assertion: cell.evidence.find(item => item.status === cell.status)?.assertion,
  }])),
}))
writeFileSync(new URL('./inventory.json', import.meta.url), `${JSON.stringify({ commit: evidence.commit, runUrl: evidence.runUrl, members }, null, 2)}\n`)

export default defineConfig({
  base: './',
  plugins: [tailwindcss()],
  server: { host: '127.0.0.1', port: 1420, strictPort: true, watch: { ignored: ['**/src-tauri/**'] } },
  build: { target: 'es2022' },
})
