import assert from 'node:assert/strict'
import { chromium } from 'playwright'
const base = 'http://127.0.0.1:4173'
const { token } = await fetch(`${base}/api/session`).then(r => r.json())
const response = await fetch(`${base}/api/accounts/subscription-review`, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json', 'X-Mailroom-Token': token },
  body: JSON.stringify({ command: { action: 'open', as_of: Math.floor(Date.now() / 1000) } }),
})
assert.equal(response.status, 200)
const view = await response.json()
assert.equal(view.synthetic, true)
const accounts = view.accounts.filter(a => a.report.rows.length >= 4).sort((a, b) => a.account.localeCompare(b.account)).slice(0, 2)
assert.equal(accounts.length, 2, 'Two existing synthetic census accounts are required')
const senders = ['alpha@example.test', 'beta@example.test', 'delta@example.test', 'gamma@example.test']
const volumes = [[3, 10, 20, 5], [7, 30, 20, 0]]
const fixture = {
  ...view,
  accounts: accounts.map((account, i) => ({
    ...account,
    report: { ...account.report, rows: account.report.rows.slice(0, 4).map((row, j) => ({
      ...row, sender: senders[j], unread_volume: volumes[i][j],
    })) },
  })).reverse(),
}
const browser = await chromium.launch({ headless: true })
try {
  const page = await browser.newPage()
  await page.route('**/api/accounts/subscription-review', async route => {
    assert.equal(route.request().postDataJSON().command.action, 'open', 'Ordering fixture never authorizes an effect')
    await route.fulfill({ json: fixture })
  })
  await page.goto(new URL('/advanced.html', base).href)
  const region = page.locator('.accounts .unsubscribe')
  await region.locator('.sender-group').first().waitFor()
  const observed = await region.locator('.sender-group').evaluateAll(groups => groups.map(group => ({
    sender: group.dataset.sender,
    rows: Array.from(group.querySelectorAll('article')).map(row => ({
      account: row.dataset.account, id: row.dataset.subscription,
      unread: Number(row.textContent.match(/unread (\d+)/)[1]),
      selected: row.querySelector('input[type=checkbox]').checked,
      method: row.querySelector('select').value,
    })),
  })))
  console.log('Observed built sender unread totals', JSON.stringify(observed.map(g => g.rows.reduce((n, r) => n + r.unread, 0))))
  assert.deepEqual(observed.map(g => g.sender), [senders[1], senders[2], senders[0], senders[3]], 'Sender groups rank by aggregate unread volume with sender tie-break')
  assert.deepEqual(observed.map(g => g.rows.map(r => r.unread)), [[30, 10], [20, 20], [7, 3], [5, 0]], 'Child subscriptions rank by unread volume')
  assert.deepEqual(observed[1].rows.map(r => r.account), accounts.map(a => a.account), 'Tied child rows use exact account/subscription key')
  for (const group of observed) for (const row of group.rows) {
    const source = fixture.accounts.find(a => a.account === row.account).report.rows.find(r => r.id === row.id)
    assert.equal(row.selected, source.recommended, 'Sorting preserves exact initial eligibility')
    assert.equal(row.method, source.methods.find(m => ['one_click', 'browser_preparation', 'mail_app_preparation'].includes(m)) ?? 'keep')
  }
  await Promise.all([
    page.waitForResponse(r => r.url().endsWith('/api/accounts/subscription-review')),
    region.getByRole('button', { name: 'Refresh subscription facts', exact: true }).click(),
  ])
  assert.deepEqual(await region.locator('.sender-group').evaluateAll(groups => groups.map(g => g.dataset.sender)), observed.map(g => g.sender), 'Refresh keeps deterministic group order')
  console.log('PASS built unread ranking, alphabetical input, aggregate cross-account totals, sender and exact-child ties, descending children, unchanged eligibility/methods and refresh')
} finally { await browser.close() }
