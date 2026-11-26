import { useContext } from "react";
import { LanguageContext } from "./components";
import type { Language } from "./components";

export function localeFor(language: Language): string {
  return language === "de" ? "de-DE" : "en-US";
}

export function useLocale(): string {
  return localeFor(useContext(LanguageContext));
}

export function formatNumber(value: number, language: Language): string {
  return value.toLocaleString(localeFor(language));
}

export function formatDateTime(unixSecs: number, language: Language): string {
  if (!unixSecs) return "";
  try {
    return new Date(unixSecs * 1000).toLocaleString(localeFor(language));
  } catch {
    return "";
  }
}

export function formatShortDate(unixSecs: number, language: Language): string {
  if (!unixSecs) return "";
  try {
    return new Date(unixSecs * 1000).toLocaleDateString(localeFor(language), {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  } catch {
    return "";
  }
}

export function pluralCount(
  count: number,
  language: Language,
  enOne: string,
  enOther: string,
  deOne: string,
  deOther: string,
): string {
  const num = formatNumber(count, language);
  if (language === "de") {
    return `${num} ${count === 1 ? deOne : deOther}`;
  }
  return `${num} ${count === 1 ? enOne : enOther}`;
}
