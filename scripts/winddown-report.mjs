import { z } from '../ui/node_modules/zod/index.js'
const args = process.argv.slice(2)
if (args.length !== 4 || args[0] !== '--account' || args[2] !== '--as-of') throw Error('Usage: node scripts/winddown-report.mjs --account ACCOUNT --as-of UTC_SECONDS')
const account = z.string().regex(/^[A-Za-z0-9-]{1,64}$/).parse(args[1])
const as_of = z.coerce.number().int().parse(args[3])
const base = 'http://127.0.0.1:4173'
const session = await fetch(`${base}/api/session`)
if (!session.ok) throw Error('Local session unavailable')
const { token } = z.object({ token: z.string() }).parse(await session.json())
const response = await fetch(`${base}/api/accounts/winddown`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token }, body: JSON.stringify({ account, as_of, command: { action: 'open' } }) })
if (!response.ok) throw Error('Local report unavailable. Open the local interface and check evidence and recovery feedback.')
process.stdout.write(z.object({ terminal: z.string() }).parse(await response.json()).terminal)
