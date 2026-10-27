import { defineConfig } from 'tsdown'

export default defineConfig({
  fixedExtension: true,
  entry: ['src-ts/**/*.ts'],
  format: ['cjs'],
  clean: true,
  dts: false,
  outDir: 'assets',
  deps: { neverBundle: ['electron'] },
})
