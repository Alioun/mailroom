import { useEffect, useState } from "react";
import type { Page } from "./AccountWorkspace";

export type ThemePreference = "system" | "light" | "dark";

export const pages: Page[] = [
  "overview",
  "senders",
  "recommendations",
  "rules",
  "backlog",
  "invoices",
  "subscriptions",
  "addresses",
  "activity",
  "settings",
];

function safeStorageGet(storage: Storage, key: string): string | null {
  try {
    return storage.getItem(key);
  } catch {
    return null;
  }
}

function safeStorageSet(storage: Storage, key: string, value: string | null) {
  try {
    if (value === null) storage.removeItem(key);
    else storage.setItem(key, value);
  } catch {
    // Private browsing or disabled web storage: keep working in-memory.
  }
}

export function stored(key: string): string | null {
  return safeStorageGet(localStorage, key);
}

export function remember(key: string, value: string): void {
  safeStorageSet(localStorage, key, value);
}

export function initialPage(): Page {
  const value = new URLSearchParams(location.search).get("view");
  return pages.find((page) => page === value) ?? "overview";
}

export function initialAccount(): string {
  const fromUrl = new URLSearchParams(location.search).get("account");
  return fromUrl ?? stored("mailroom.account") ?? "";
}

export function initialTheme(): ThemePreference {
  const value = stored("mailroom.theme");
  return value === "light" || value === "dark" || value === "system"
    ? value
    : "system";
}

export function pendingAutoScan(): boolean {
  return safeStorageGet(sessionStorage, "mailroom.autoscan") === "waiting";
}

export function autoScan(value: boolean): void {
  safeStorageSet(sessionStorage, "mailroom.autoscan", value ? "waiting" : null);
}

export function cycleThemePreference(current: ThemePreference): ThemePreference {
  const order: ThemePreference[] = ["system", "light", "dark"];
  return order[(order.indexOf(current) + 1) % order.length];
}

export function useThemePreference() {
  const [theme, setTheme] = useState<ThemePreference>(initialTheme);

  useEffect(() => {
    remember("mailroom.theme", theme);
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const resolved =
        theme === "system" ? (mq.matches ? "dark" : "light") : theme;
      document.documentElement.dataset.theme = resolved;
    };
    apply();
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [theme]);

  return [theme, setTheme] as const;
}

export function useShortcutsEnabled() {
  const [enabled, setEnabled] = useState<boolean>(
    () => stored("mailroom.shortcuts") !== "off",
  );

  useEffect(() => {
    remember("mailroom.shortcuts", enabled ? "on" : "off");
  }, [enabled]);

  return [enabled, setEnabled] as const;
}
