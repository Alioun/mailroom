import { z } from 'zod'

// Zod probes `new Function` when it builds each object schema unless jitless is set
// first; under the production CSP that probe is a reported script-src violation.
z.config({ jitless: true })
