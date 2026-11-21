import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  writeFile,
} from "node:fs/promises";
import { resolve, join } from "node:path";
import { chromium } from "playwright";

const binary = resolve(
  process.argv[2] ?? "../sender-catalog-target/debug/mailroom",
);
const artifacts = resolve(process.argv[3] ?? ".audit/workbench-browser");
await mkdir(artifacts, { recursive: true, mode: 0o700 });
const home = await mkdtemp(join(artifacts, "home-"));
await chmod(home, 0o700);
const temp = join(home, "tmp");
await mkdir(temp, { mode: 0o700 });
const origin = "http://127.0.0.1:4173";
assert.equal(
  await fetch(`${origin}/api/session`).then(
    () => true,
    () => false,
  ),
  false,
  "Port 4173 must be free; never replace an owner server",
);
const checks = [],
  errors = [],
  violations = [];
function check(name) {
  checks.push(name);
  console.log(`ok ${name}`);
}
const browser = await chromium.launch({
  headless: true,
  env: { HOME: process.env.HOME, PATH: "/usr/bin:/bin:/usr/sbin:/sbin" },
});
let server;
async function start(mode) {
  let output = "";
  const child = spawn(binary, [mode], {
    env: { HOME: home, TMPDIR: temp, PATH: "/usr/bin:/bin" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stdout.on("data", (bytes) => {
    output += bytes;
  });
  child.stderr.on("data", (bytes) => {
    output += bytes;
  });
  const exited = once(child, "exit");
  for (let attempt = 0; attempt < 150; attempt++) {
    if (child.exitCode !== null)
      throw Error(`Test server exited ${child.exitCode}: ${output}`);
    if (
      await fetch(`${origin}/api/session`).then(
        (r) => r.ok,
        () => false,
      )
    )
      return { child, exited };
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  child.kill();
  throw Error("Test server did not start");
}
async function stop() {
  if (server) {
    server.child.kill("SIGTERM");
    await server.exited;
    server = undefined;
  }
}
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
page.on("pageerror", (error) => errors.push(error.message));
await page.addInitScript(() => {
  document.addEventListener("securitypolicyviolation", (event) => {
    window.__cspViolations ??= [];
    window.__cspViolations.push(event.violatedDirective);
  });
});
async function goto() {
  await page.goto(origin);
  await page.locator("h1").waitFor();
}
async function language(value) {
  await page.getByRole("combobox", { name: /Language|Sprache/ }).click();
  await page.getByRole("option", { name: value, exact: true }).click();
}
async function navigate(name) {
  if (
    await page
      .getByRole("button", { name: "Open navigation", exact: true })
      .isVisible()
  )
    await page
      .getByRole("button", { name: "Open navigation", exact: true })
      .click();
  await page
    .locator(".sidebar")
    .getByRole("button", { name, exact: true })
    .click();
}
async function noOverflow(label) {
  assert.ok(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth + 1,
    ),
    `${label} has no page-wide overflow`,
  );
}
async function api(path, body) {
  return page.evaluate(
    async ({ path, body }) => {
      const { token } = await (await fetch("/api/session")).json();
      const response = await fetch(`/api/accounts/${path}`, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-mailroom-token": token,
        },
        body: JSON.stringify(body),
      });
      return { status: response.status, value: await response.json() };
    },
    { path, body },
  );
}
async function findExchanges(directory) {
  const found = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const path = join(directory, entry.name);
    if (entry.name === "exchange") found.push(path);
    else found.push(...(await findExchanges(path)));
  }
  return found;
}
function mcp(command, folder, input) {
  const result = spawnSync(binary, ["agent", command, "--folder", folder], {
    env: { HOME: home, PATH: "/usr/bin:/bin" },
    input: input ? JSON.stringify(input) : undefined,
    encoding: "utf8",
    timeout: 15000,
  });
  assert.equal(result.status, 0, `MCP ${command} succeeded`);
  const value = JSON.parse(result.stdout);
  assert.equal(value.isError, false);
  return JSON.parse(value.content[0].text);
}
try {
  server = await start("--local-vault");
  await goto();
  await page
    .getByRole("heading", { name: "Welcome to Mailroom", exact: true })
    .waitFor();
  const master = "invented browser vault password 902a";
  await page.getByLabel("Master password", { exact: true }).fill(master);
  await page.getByLabel("Repeat password", { exact: true }).fill(master + "x");
  await page.getByRole("checkbox").check();
  await page.getByRole("button", { name: "Create vault", exact: true }).click();
  await page
    .getByRole("alert")
    .getByText("The passwords do not match.")
    .waitFor();
  await page.getByLabel("Repeat password", { exact: true }).fill(master);
  await page.getByRole("button", { name: "Create vault", exact: true }).click();
  await page
    .getByRole("button", { name: "Connect and scan", exact: true })
    .waitFor();
  assert.ok(!(await page.content()).includes(master));
  check(
    "browser creates an encrypted vault, associates mismatch feedback, and clears credentials",
  );
  await stop();
  server = await start("--local-vault");
  await page.reload();
  await page
    .getByRole("heading", { name: "Welcome back", exact: true })
    .waitFor();
  await page
    .getByLabel("Master password", { exact: true })
    .fill("invented wrong browser password");
  await page
    .getByRole("button", { name: "Unlock Mailroom", exact: true })
    .click();
  await page
    .getByText("That password did not open the vault. Try again.", {
      exact: true,
    })
    .waitFor();
  assert.equal(
    await page.getByLabel("Master password", { exact: true }).inputValue(),
    "",
  );
  await page.route("**/api/accounts/list", (route) =>
    route.fulfill({
      json: [
        {
          id: "returning-synthetic",
          provider: "mailbox_org",
          login: "returning@example.test",
          connection: "observed",
        },
      ],
    }),
  );
  await page.getByLabel("Master password", { exact: true }).fill(master);
  await page
    .getByRole("button", { name: "Unlock Mailroom", exact: true })
    .click();
  await page
    .getByRole("combobox", { name: "Select account", exact: true })
    .getByText("returning@example.test · mailbox.org", { exact: true })
    .waitFor();
  assert.equal(
    await page
      .getByRole("button", { name: "Connect and scan", exact: true })
      .count(),
    0,
  );
  await page.unrouteAll({ behavior: "wait" });
  // Verify top bar Lock button locks the vault and returns to Welcome back
  await page.getByRole("button", { name: "Lock vault", exact: true }).click();
  await page
    .getByRole("heading", { name: "Welcome back", exact: true })
    .waitFor();
  await page.getByLabel("Master password", { exact: true }).fill(master);
  await page
    .getByRole("button", { name: "Unlock Mailroom", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Connect and scan", exact: true })
    .waitFor();
  const storage = await page.evaluate(() =>
    JSON.stringify({
      local: { ...localStorage },
      session: { ...sessionStorage },
    }),
  );
  assert.ok(!storage.includes(master) && !storage.includes("wrong browser"));
  check(
    "restart unlock loads saved-account listing, top-bar lock/unlock works, and wrong-password recovery leaves no password in storage",
  );
  // Verify /advanced.html is retired
  const advStatus = await fetch(`${origin}/advanced.html`).then((r) => r.status);
  assert.equal(advStatus, 404, "/advanced.html must be retired");
  check("/advanced.html is retired (404)");
  await stop();
  server = await start("--synthetic-accounts");
  await page.reload();
  await page
    .getByRole("button", { name: "Use demo account", exact: true })
    .click();
  await page
    .getByText("Scan complete", { exact: true })
    .filter({ visible: true })
    .first()
    .waitFor({ timeout: 60000 });
  await page
    .getByRole("heading", { name: "Mailbox activity", exact: true })
    .waitFor();
  assert.equal(
    await page.locator(".stats .stat").first().locator("strong").textContent(),
    "100",
  );
  check(
    "connect automatically starts the census and opens readable findings without report generation",
  );
  await page.setViewportSize({ width: 667, height: 375 });
  await page
    .getByRole("button", { name: "Open navigation", exact: true })
    .click();
  const shortSettings = page
    .locator(".mobile-drawer")
    .getByRole("button", { name: "Settings", exact: true });
  await shortSettings.scrollIntoViewIfNeeded();
  const settingsBox = await shortSettings.boundingBox();
  assert.ok(
    settingsBox &&
      settingsBox.y >= 0 &&
      settingsBox.y + settingsBox.height <= 375,
    "Settings remains reachable in a short landscape drawer",
  );
  await shortSettings.click();
  await page.getByRole("heading", { name: "Settings", exact: true }).waitFor();
  await page.setViewportSize({ width: 1440, height: 1000 });
  await navigate("Overview");
  check("short landscape navigation scrolls to Settings");
  await page.screenshot({
    path: join(artifacts, "overview-desktop.png"),
    fullPage: true,
  });
  await navigate("Senders");
  await page
    .getByRole("textbox", { name: "Search senders", exact: true })
    .fill("does-not-exist");
  await page
    .getByRole("heading", { name: "No matching senders", exact: true })
    .waitFor();
  await navigate("Overview");
  await navigate("Senders");
  assert.equal(
    await page
      .getByRole("textbox", { name: "Search senders", exact: true })
      .inputValue(),
    "does-not-exist",
  );
  await page
    .getByRole("textbox", { name: "Search senders", exact: true })
    .fill("");
  check("task navigation preserves sender search state");
  const reportResponse = await api("report", {
    account: (await api("list", {})).value[0].id,
    as_of: Math.floor(Date.now() / 1000),
  });
  assert.equal(reportResponse.status, 200);
  const fixture = structuredClone(reportResponse.value),
    template = fixture.senders[0];
  fixture.senders = Array.from({ length: 1000 }, (_, i) => ({
    ...template,
    sender: `sender-${String(i).padStart(4, "0")}@example.test`,
    domain: "example.test",
    class: i % 2 ? "bulk" : "platform",
    counts: { ...template.counts, messages: i + 1, seen: 0 },
  }));
  await page.route("**/api/accounts/report", (route) =>
    route.fulfill({ json: fixture }),
  );
  await page.reload();
  await page
    .getByRole("heading", { name: "Senders", exact: true })
    .first()
    .waitFor();
  await page
    .getByRole("textbox", { name: "Search senders", exact: true })
    .fill("sender-0999");
  await page
    .locator(".senders-table")
    .getByText("sender-0999@example.test", { exact: true })
    .waitFor();
  assert.equal(await page.locator(".senders-table tbody tr").count(), 1);
  await page
    .getByRole("textbox", { name: "Search senders", exact: true })
    .fill("");
  assert.equal(await page.locator(".senders-table tbody tr").count(), 20);
  await page
    .getByRole("combobox", { name: "Filter by type", exact: true })
    .click();
  await page.getByRole("option", { name: "Bulk mail", exact: true }).click();
  await page.getByRole("button", { name: "Next page", exact: true }).click();
  assert.ok((await page.locator(".pagination").textContent()).includes("21"));
  await page
    .locator(".senders-table tbody tr")
    .first()
    .getByRole("button")
    .click();
  await page.getByRole("dialog").waitFor();
  await page.keyboard.press("Escape");
  await page.getByRole("dialog").waitFor({ state: "hidden" });
  await page.waitForFunction(() =>
    document.activeElement?.matches(".senders-table button"),
  );
  assert.ok(
    await page
      .locator(".senders-table tbody tr")
      .first()
      .getByRole("button")
      .evaluate((node) => node === document.activeElement),
  );
  check(
    "1000 senders support search, filtering, pagination, and dialog focus recovery",
  );
  await page.unroute("**/api/accounts/report");
  await page.reload();
  await navigate("Rules & deploy");
  await page
    .getByRole("button", { name: "Create local rules", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Check saved rules", exact: true })
    .waitFor();
  await page
    .getByRole("button", { name: "Check saved rules", exact: true })
    .click();
  await page
    .getByRole("heading", { name: "Simulation on saved mail", exact: true })
    .waitFor();
  await page.getByText("Edit exact rule documents", { exact: true }).click();
  const original = await page
    .getByLabel("Override YAML", { exact: true })
    .inputValue();
  await page
    .getByLabel("Override YAML", { exact: true })
    .fill(original + "\n# invented draft");
  await navigate("Overview");
  await navigate("Rules & deploy");
  assert.ok(
    (
      await page.getByLabel("Override YAML", { exact: true }).inputValue()
    ).includes("invented draft"),
  );
  const firstAccount = (await api("list", {})).value[0];
  const createSecond = await api("synthetic-gmail", {});
  assert.equal(createSecond.status, 200);
  await page.reload();
  await page
    .getByRole("combobox", { name: "Select account", exact: true })
    .click();
  await page.getByRole("option", { name: /Gmail/ }).click();
  await page
    .getByRole("combobox", { name: "Select account", exact: true })
    .click();
  await page
    .getByRole("option", {
      name: `${firstAccount.login} · mailbox.org`,
      exact: true,
    })
    .click();
  await page.getByText("Edit exact rule documents", { exact: true }).click();
  await page
    .getByLabel("Override YAML", { exact: true })
    .fill(original + "\n# account switch draft");
  await page
    .getByRole("combobox", { name: "Select account", exact: true })
    .click();
  await page.getByRole("option", { name: /Gmail/ }).click();
  await page
    .getByRole("combobox", { name: "Select account", exact: true })
    .click();
  await page
    .getByRole("option", {
      name: `${firstAccount.login} · mailbox.org`,
      exact: true,
    })
    .click();
  assert.ok(
    (
      await page.getByLabel("Override YAML", { exact: true }).inputValue()
    ).includes("account switch draft"),
  );
  assert.equal(
    await page
      .getByRole("button", { name: "Save local rules", exact: true })
      .isEnabled(),
    true,
  );
  await page
    .getByRole("button", { name: "Load saved rules", exact: true })
    .click();
  check(
    "local rule creation and simulation work, and navigation preserves unsaved drafts",
  );
  await page
    .getByRole("combobox", { name: "Select account", exact: true })
    .click();
  await page.getByRole("option", { name: /Gmail/ }).click();
  await navigate("Overview");
  await page.getByRole("button", { name: "Scan mailbox", exact: true }).click();
  await page
    .getByText("Scan complete", { exact: true })
    .filter({ visible: true })
    .first()
    .waitFor({ timeout: 60000 });
  const secondAccount = (await api("list", {})).value.find(
    (value) => value.provider === "gmail",
  );
  await page
    .getByRole("combobox", { name: "Select account", exact: true })
    .click();
  await page
    .getByRole("option", {
      name: `${firstAccount.login} · mailbox.org`,
      exact: true,
    })
    .click();
  await page.route("**/api/accounts/status", async (route) => {
    const response = await route.fetch();
    const failedStatus = await response.json();
    failedStatus.job = {
      operation: "connect",
      account: secondAccount,
      progress: { state: "failed", saved: 0, folders: 0 },
      error: "invented failure",
    };
    await route.fulfill({ json: failedStatus });
  });
  await page
    .getByRole("button", { name: "Connection settings", exact: true })
    .click();
  assert.ok(
    (
      await page
        .getByRole("combobox", { name: "Select account", exact: true })
        .textContent()
    ).includes("Gmail"),
  );
  await page.unrouteAll({ behavior: "wait" });
  check(
    "failed additional-account recovery selects the failed account before opening connection settings",
  );
  await navigate("Recommendations");
  let launchRequests = 0;
  await page.route("**/api/accounts/assistant", async (route) => {
    const body = route.request().postDataJSON();
    if (body.command.action === "launch") {
      const response = await route.fetch({
        postData: JSON.stringify({
          account: body.account,
          command: { action: "status" },
        }),
      });
      await route.fulfill({ response });
      launchRequests++;
    } else await route.continue();
  });
  await page
    .getByRole("button", { name: "Get recommendations", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByText("Review what T3 will receive", { exact: true })
    .waitFor();
  await page.locator(".summary-preview").waitFor();
  assert.equal(await page.locator(".domain-options [data-checked]").count(), 0);
  const launchResponse = page.waitForResponse(
    (response) =>
      response.url().endsWith("/api/accounts/assistant") &&
      response.request().postDataJSON()?.command?.action === "launch",
  );
  await page
    .getByRole("button", { name: "Share and open T3", exact: true })
    .click();
  await launchResponse;
  await page.getByText("Ready to send", { exact: true }).waitFor();
  assert.equal(launchRequests, 1);
  await page.unrouteAll({ behavior: "wait" });
  const exchanges = await findExchanges(temp);
  assert.equal(exchanges.length, 1);
  const reviewed = mcp("read", exchanges[0]);
  const proposal = {
    version: 1,
    summary_sha256: reviewed.summary_sha256,
    recommendations: [
      {
        title: "Review travel filing",
        reason: "Invented browser verification. <img src=x onerror=alert(1)>",
        suggested_change: "Place travel matches in the TravelReview folder.",
      },
    ],
    rule_edits: { patches: [{ id: "topic.travel", folder: "TravelReview" }] },
  };
  mcp("submit", exchanges[0], proposal);
  mcp("submit", exchanges[0], proposal);
  await page
    .getByText("Review travel filing", { exact: true })
    .waitFor({ timeout: 10000 });
  assert.equal(
    await page
      .getByRole("button", { name: "Review proposal", exact: true })
      .count(),
    1,
  );
  await page.reload();
  await page
    .getByRole("button", { name: "Review proposal", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Create local rules", exact: true })
    .click();
  await page
    .getByRole("heading", { name: "Effect on saved mail", exact: true })
    .waitFor();
  assert.equal(await page.locator(".recommendation-list img").count(), 0);
  await page
    .getByRole("button", { name: "Save local rules", exact: true })
    .click();
  await page
    .getByText(
      "Local rules saved. Their updated simulation is available in Local rules.",
      { exact: true },
    )
    .waitFor();
  await navigate("Rules & deploy");
  await page.getByText("Edit exact rule documents", { exact: true }).click();
  assert.ok(
    (
      await page.getByLabel("Override YAML", { exact: true }).inputValue()
    ).includes("TravelReview"),
  );
  // Check Rules & deploy sub-tabs
  for (const sub of ["Folders", "Sieve deploy", "Placement check", "Local rules & proof"]) {
    await page.getByRole("tab", { name: sub, exact: true }).click();
  }
  check(
    "browser shares the exact masked summary, real MCP read/submit arrives automatically, retries deduplicate, saving updates local rules, and all Rules & deploy sub-tabs render",
  );
  // Exercise unified Organize views (Backlog, Invoices, Subscriptions, Addresses & wind-down)
  await navigate("Backlog");
  await page.getByRole("heading", { name: "Backlog", exact: true }).waitFor();
  await navigate("Invoices");
  await page.getByRole("heading", { name: "Invoices", exact: true }).waitFor();
  await navigate("Subscriptions");
  await page.getByRole("heading", { name: "Subscriptions", exact: true }).waitFor();
  await navigate("Addresses & wind-down");
  await page
    .getByRole("heading", { name: "Addresses & wind-down", exact: true })
    .waitFor();
  for (const sub of ["Gmail forwarding", "Gmail wind-down", "Address ledger"]) {
    await page.getByRole("tab", { name: sub, exact: true }).click();
  }
  check("unified Backlog, Invoices, Subscriptions, and Addresses & wind-down views render cleanly");
  // Exercise Settings: Safety Mode toggle, Takeout .mbox import, T3 provider/model config
  await navigate("Settings");
  await page
    .getByRole("button", { name: "Enable live writes for this session", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("checkbox")
    .check();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Enable live writes", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Return to read-only", exact: true })
    .waitFor();
  await page
    .getByRole("button", { name: "Return to read-only", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Enable live writes for this session", exact: true })
    .waitFor();
  const mboxPath = join(temp, "sample-takeout.mbox");
  await writeFile(
    mboxPath,
    [
      "From 1@xxx Sat Nov 21 00:00:00 +0000 2026",
      "X-Gmail-Labels: Inbox,Unread",
      "From: Bank <info@dkb.de>",
      "To: owner@example.test",
      "Subject: Statement",
      "Date: Sat, 21 Nov 2026 00:00:00 +0000",
      "",
      "PRIVATE-BODY-NEVER-STORED",
      "",
    ].join("\n"),
  );
  await page
    .getByLabel("Local .mbox file path", { exact: true })
    .fill(mboxPath);
  await page
    .getByRole("button", { name: "Import .mbox headers", exact: true })
    .click();
  await page.getByText(/Last import: 1 imported/).waitFor();
  await page.getByLabel("T3 provider", { exact: true }).fill("anthropic");
  await page.getByLabel("T3 model", { exact: true }).fill("claude-opus");
  await page.getByRole("button", { name: "Save T3 settings", exact: true }).click();
  await page.getByText("T3 assistant configuration saved.", { exact: true }).waitFor();
  await page.getByRole("button", { name: "Reset defaults", exact: true }).click();
  await page.getByRole("button", { name: "Save T3 settings", exact: true }).click();
  check("Settings Safety Mode toggle, local Takeout .mbox import, and T3 provider/model config work end-to-end");
  await page.setViewportSize({ width: 320, height: 900 });
  await page
    .getByRole("button", { name: "Open navigation", exact: true })
    .waitFor();
  await page.locator(".sidebar").waitFor({ state: "detached" });
  assert.equal(await page.locator(".sidebar").count(), 0);
  await page
    .getByRole("button", { name: "Open navigation", exact: true })
    .click();
  await page.waitForFunction(() =>
    document.activeElement?.closest(".mobile-drawer"),
  );
  await page.keyboard.press("Shift+Tab");
  await page.waitForFunction(
    () => document.activeElement?.closest(".mobile-drawer"),
    null,
    { timeout: 3000 },
  );
  assert.ok(
    await page.evaluate(
      () => document.activeElement?.closest(".mobile-drawer") !== null,
    ),
  );
  for (let step = 0; step < 14; step++) {
    await page.keyboard.press("Tab");
    await page.waitForFunction(
      () => document.activeElement?.closest(".mobile-drawer"),
      null,
      { timeout: 3000 },
    );
    assert.ok(
      await page.evaluate(
        () => document.activeElement?.closest(".mobile-drawer") !== null,
      ),
    );
  }
  await page.keyboard.press("Escape");
  await page.waitForFunction(
    () =>
      document.activeElement?.getAttribute("aria-label") === "Open navigation",
  );
  await page.locator(".sidebar").waitFor({ state: "detached" });
  assert.equal(await page.locator(".sidebar").count(), 0);
  check(
    "mobile navigation has no hidden tab stops, traps focus while open, and restores the menu trigger",
  );
  for (const width of [320, 768, 1440]) {
    await page.setViewportSize({ width, height: 900 });
    for (const name of [
      "Overview",
      "Senders",
      "Recommendations",
      "Rules & deploy",
      "Backlog",
      "Invoices",
      "Subscriptions",
      "Addresses & wind-down",
      "Settings",
    ]) {
      await navigate(name);
      await noOverflow(`${name} at ${width}`);
    }
  }
  await page.setViewportSize({ width: 320, height: 900 });
  await navigate("Overview");
  await language("Deutsch");
  await noOverflow("German overview at 320");
  await page.screenshot({
    path: join(artifacts, "overview-mobile-de.png"),
    fullPage: true,
  });
  await page
    .getByRole("button", { name: "Navigation öffnen", exact: true })
    .click();
  await page
    .locator(".sidebar")
    .getByRole("button", { name: "Vorschläge", exact: true })
    .click();
  await page
    .getByRole("button", {
      name: "Gespeicherten Vorschlag ansehen",
      exact: true,
    })
    .click();
  await noOverflow("German proposal at 320");
  await page.keyboard.press("Escape");
  check(
    "all core pages fit 320, 768, and 1440 pixels, including German proposal review",
  );
  assert.deepEqual(errors, []);
  violations.push(...(await page.evaluate(() => window.__cspViolations ?? [])));
  assert.deepEqual(violations, []);
  const denied = await api("backlog", {
    account: (await api("list", {})).value[0].id,
    command: { action: "execute" },
  });
  assert.notEqual(denied.status, 200);
  check(
    "no browser runtime or CSP errors and direct provider mutation remains refused",
  );
  await writeFile(
    join(artifacts, "results.json"),
    JSON.stringify(
      {
        checks,
        mountedBrowser: "passed",
        t3Launch: "intercepted; adapter covered by real dummy-CLI Rust tests",
        errors,
        violations,
      },
      null,
      2,
    ) + "\n",
  );
} catch (error) {
  await page
    .screenshot({ path: join(artifacts, "failure.png"), fullPage: true })
    .catch(() => {});
  await writeFile(
    join(artifacts, "failure-dom.txt"),
    await page.locator("body").innerText(),
  ).catch(() => {});
  throw error;
} finally {
  await browser.close();
  await stop();
}
