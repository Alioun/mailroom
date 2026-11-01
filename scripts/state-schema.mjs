import { readFileSync } from 'node:fs'

const source = readFileSync(new URL('../src/census/store.rs', import.meta.url), 'utf8')
const match = source.match(/^pub\(crate\) const SCHEMA: u32 = (\d+);$/m)
if (!match) throw new Error('src/census/store.rs no longer declares the account state SCHEMA')
export const stateSchema = Number(match[1])
