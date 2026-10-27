import { defineConfig } from 'tsdown'

export default defineConfig({
  fixedExtension: true,
  entry: ['src-ts/**/*.ts'],
  root: 'src-ts',
  unbundle: true,
  platform: 'node',
  target: 'node24',
  format: ['cjs'],
  clean: true,
  dts: false,
  minify: true,
  treeshake: false,
  outDir: 'assets',
  deps: { neverBundle: ['electron'] },
})
