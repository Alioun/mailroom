import type { Preview } from './api'

export type ProofFilter = 'all' | 'inbox' | 'copies' | 'review'
export const PROOF_PAGE_SIZE = 50

export function proofResults(proof: Pick<Preview, 'decisions' | 'diff'>, filter: ProofFilter, query: string, requestedPage: number) {
  const term = query.trim().normalize('NFC').toLowerCase()
  const matches = proof.decisions.filter(decision => {
    const included = filter === 'review' ? decision.review : filter === 'inbox' ? decision.destination.kind === 'inbox' : filter === 'copies' ? decision.copies.length > 0 : true
    const destination = decision.destination.kind === 'inbox' ? 'INBOX' : decision.destination.name
    return included && (!term || [decision.subject, decision.from, destination].some(value => value.normalize('NFC').toLowerCase().includes(term)))
  })
  const pages = Math.max(1, Math.ceil(matches.length / PROOF_PAGE_SIZE))
  const page = Math.min(Math.max(0, requestedPage), pages - 1)
  const decisions = matches.slice(page * PROOF_PAGE_SIZE, (page + 1) * PROOF_PAGE_SIZE)
  const ids = new Set(decisions.map(decision => decision.id))
  return {
    page, pages, decisions, total: matches.length,
    first: matches.length ? page * PROOF_PAGE_SIZE + 1 : 0,
    last: Math.min((page + 1) * PROOF_PAGE_SIZE, matches.length),
    diff: proof.diff.filter(change => ids.has(change.id)),
  }
}
