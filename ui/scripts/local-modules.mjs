import { existsSync, readFileSync } from 'node:fs'

const source = new URL('../src/', import.meta.url)

// Names of the ui/src TypeScript modules the entries import, transitively, entries included.
export function localModules(entries) {
  const seen = new Set()
  const visit = name => {
    if (seen.has(name)) return
    seen.add(name)
    for (const [, local] of readFileSync(new URL(`${name}.ts`, source), 'utf8').matchAll(/from '\.\/([^']+)'/g)) {
      if (existsSync(new URL(`${local}.ts`, source))) visit(local)
    }
  }
  entries.forEach(visit)
  return [...seen]
}
