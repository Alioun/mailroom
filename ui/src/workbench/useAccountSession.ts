import { useCallback, useEffect, useRef, useState } from "react";
import { listAccounts, pollAccountEvents, scanAccount } from "../api";
import type { Account, AccountStatus } from "../api";
import type { Page } from "./AccountWorkspace";
import {
  autoScan,
  initialAccount,
  initialPage,
  pages,
  pendingAutoScan,
  remember,
} from "./preferences";

export function useAccountSession(t: (en: string, de: string) => string) {
  const [status, setStatus] = useState<AccountStatus | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [selected, setSelected] = useState(initialAccount);
  const [page, setPage] = useState<Page>(initialPage);
  const [error, setError] = useState("");
  const [refresh, setRefresh] = useState(0);
  const [liveCountdown, setLiveCountdown] = useState<number | null>(null);
  const lastJob = useRef<string | null>(null);
  const eventSeq = useRef<number | undefined>(undefined);
  const wasUsable = useRef(false);

  const loadAccounts = useCallback(async () => {
    const values = await listAccounts();
    setAccounts(values);
    setSelected((current) =>
      values.some((value) => value.id === current)
        ? current
        : (values[0]?.id ?? ""),
    );
    return values;
  }, []);

  const applyStatus = useCallback(
    async (next: AccountStatus) => {
      setStatus(next);
      setLiveCountdown(
        next.live_writes ? (next.live_writes_expires_in_secs ?? null) : null,
      );
      const job = next.job;
      const signature = job
        ? `${job.account?.id}-${job.progress.state}-${job.operation ?? ""}`
        : "none";
      const usable =
        next.vault !== "locked" &&
        next.vault !== "absent" &&
        next.vault !== "unavailable";
      const becameUsable = usable && !wasUsable.current;
      wasUsable.current = usable;
      if (signature !== lastJob.current || becameUsable) {
        const previous = lastJob.current;
        lastJob.current = signature;
        if (usable) {
          await loadAccounts();
          if (previous !== null && job?.progress.state !== "running")
            setRefresh((value) => value + 1);
        }
      }
      if (
        pendingAutoScan() &&
        job?.progress.state === "complete" &&
        (job.operation === "connect" || job.operation === "reconnect") &&
        job.account
      ) {
        autoScan(false);
        setSelected(job.account.id);
        setPage("overview");
        setStatus(await scanAccount(job.account.id));
        setRefresh((value) => value + 1);
      }
    },
    [loadAccounts],
  );

  // Instant push updates via /api/accounts/events long-poll loop with backoff on disconnect.
  useEffect(() => {
    let active = true;
    let controller = new AbortController();

    const loop = async () => {
      while (active) {
        controller = new AbortController();
        try {
          const event = await pollAccountEvents(
            eventSeq.current,
            controller.signal,
          );
          if (!active) break;
          eventSeq.current = event.seq;
          await applyStatus(event.status);
          setError("");
        } catch (err) {
          if (!active) break;
          if (err instanceof DOMException && err.name === "AbortError") break;
          setError(
            t(
              "Mailroom is not responding. Check that the app is running, then retry.",
              "Mailroom antwortet nicht. Prüfe, ob die App läuft, und versuche es erneut.",
            ),
          );
          await new Promise((resolve) => setTimeout(resolve, 2000));
        }
      }
    };
    void loop();
    return () => {
      active = false;
      controller.abort();
    };
  }, [applyStatus, t]);

  // Smooth local 1-second tick for live_writes auto-disarm countdown (paused while job runs).
  useEffect(() => {
    if (
      !status?.live_writes ||
      liveCountdown === null ||
      status.job?.progress.state === "running"
    ) {
      return;
    }
    const timer = setInterval(() => {
      setLiveCountdown((prev) => (prev !== null && prev > 0 ? prev - 1 : prev));
    }, 1000);
    return () => clearInterval(timer);
  }, [status?.live_writes, status?.job?.progress.state, liveCountdown !== null]);

  useEffect(() => {
    if (selected) remember("mailroom.account", selected);
  }, [selected]);

  useEffect(() => {
    const syncFromUrl = () => {
      const params = new URLSearchParams(location.search);
      const v = params.get("view");
      const nextPage = pages.find((p) => p === v) ?? "overview";
      setPage(nextPage);
      const acc = params.get("account");
      if (acc) setSelected(acc);
    };
    window.addEventListener("popstate", syncFromUrl);
    return () => window.removeEventListener("popstate", syncFromUrl);
  }, []);

  const pushUrl = useCallback((nextPage: Page, nextAccount: string) => {
    const query = new URLSearchParams(location.search);
    const same =
      query.get("view") === nextPage &&
      (nextAccount ? query.get("account") === nextAccount : true);
    query.set("view", nextPage);
    if (nextAccount) query.set("account", nextAccount);
    else query.delete("account");
    const target = `${location.pathname}?${query}`;
    if (!same) history.pushState(null, "", target);
  }, []);

  const bumpRefresh = useCallback(() => {
    setRefresh((value) => value + 1);
  }, []);

  return {
    status,
    accounts,
    selected,
    setSelected,
    page,
    setPage,
    error,
    setError,
    refresh,
    bumpRefresh,
    liveCountdown,
    loadAccounts,
    applyStatus,
    pushUrl,
  };
}
