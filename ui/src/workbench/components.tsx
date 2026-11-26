import {
  createContext,
  useCallback,
  useContext,
  useId,
  useRef,
  useState,
} from "react";
import type { ReactNode, ComponentProps } from "react";
import { Button } from "@base-ui/react/button";
import { Dialog } from "@base-ui/react/dialog";
import { Select } from "@base-ui/react/select";
import { Checkbox } from "@base-ui/react/checkbox";
import { Tabs } from "@base-ui/react/tabs";
import {
  Check,
  ChevronDown,
  X,
  Inbox,
  LoaderCircle,
  ShieldAlert,
  ShieldCheck,
} from "lucide-react";
import { statusMeta } from "./status-labels";
import type { BadgeTone } from "./status-labels";

export type Language = "en" | "de";
export const LanguageContext = createContext<Language>("en");
export function useCopy() {
  const language = useContext(LanguageContext);
  return useCallback(
    (en: string, de: string) => (language === "de" ? de : en),
    [language],
  );
}
export function Action({
  primary = false,
  tone,
  size = "default",
  className = "",
  ...props
}: ComponentProps<typeof Button> & {
  primary?: boolean;
  tone?: "default" | "primary" | "warning" | "danger" | "danger-outline";
  size?: "default" | "compact";
}) {
  const resolvedTone = tone ?? (primary ? "primary" : "default");
  const toneClass = resolvedTone === "default" ? "" : resolvedTone;
  const sizeClass = size === "compact" ? "compact" : "";
  return (
    <Button
      className={`button ${toneClass} ${sizeClass} ${className}`.trim()}
      {...props}
    />
  );
}
export function CheckField({
  checked,
  onChange,
  children,
  disabled = false,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  children: ReactNode;
  disabled?: boolean;
}) {
  return (
    <label className="check-field">
      <Checkbox.Root
        checked={checked}
        onCheckedChange={onChange}
        disabled={disabled}
        className="checkbox"
      >
        <Checkbox.Indicator>
          <Check size={14} />
        </Checkbox.Indicator>
      </Checkbox.Root>
      <span>{children}</span>
    </label>
  );
}
export function Choice({
  value,
  onChange,
  options,
  label,
  fieldLabel,
  disabled = false,
}: {
  value: string;
  onChange: (value: string) => void;
  options: { value: string; label: string }[];
  label: string;
  fieldLabel?: string;
  disabled?: boolean;
}) {
  const labelId = useId();
  const select = (
    <Select.Root
      value={value}
      disabled={disabled}
      onValueChange={(next) => {
        if (next !== null) onChange(next);
      }}
      items={options}
    >
      <Select.Trigger
        className="select-trigger"
        aria-label={fieldLabel ? undefined : label}
        aria-labelledby={fieldLabel ? labelId : undefined}
      >
        <Select.Value />
        <Select.Icon>
          <ChevronDown size={16} />
        </Select.Icon>
      </Select.Trigger>
      <Select.Portal>
        <Select.Positioner sideOffset={5} className="select-positioner">
          <Select.Popup className="select-popup">
            <Select.List>
              {options.map((option) => (
                <Select.Item
                  className="select-item"
                  key={option.value}
                  value={option.value}
                >
                  <Select.ItemText>{option.label}</Select.ItemText>
                  <Select.ItemIndicator>
                    <Check size={16} />
                  </Select.ItemIndicator>
                </Select.Item>
              ))}
            </Select.List>
          </Select.Popup>
        </Select.Positioner>
      </Select.Portal>
    </Select.Root>
  );
  if (!fieldLabel) return select;
  return (
    <div className="field">
      <span id={labelId} className="field-label">
        {fieldLabel}
      </span>
      {select}
    </div>
  );
}
export function Modal({
  open,
  onOpenChange,
  title,
  description,
  children,
  wide = false,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: string;
  children: ReactNode;
  wide?: boolean;
}) {
  const t = useCopy();
  const opener = useRef<HTMLElement | null>(null);
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Backdrop className="dialog-backdrop" />
        <Dialog.Popup
          className={`dialog ${wide ? "wide" : ""}`}
          initialFocus={() => {
            opener.current =
              document.activeElement instanceof HTMLElement
                ? document.activeElement
                : null;
            return true;
          }}
          finalFocus={() =>
            opener.current?.isConnected ? opener.current : true
          }
        >
          <div className="dialog-heading">
            <Dialog.Title>{title}</Dialog.Title>
            <Dialog.Close
              className="icon-button"
              aria-label={t("Close", "Schließen")}
            >
              <X size={20} />
            </Dialog.Close>
          </div>
          {description && (
            <Dialog.Description className="muted">
              {description}
            </Dialog.Description>
          )}
          {children}
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
export function Notice({
  children,
  error = false,
  tone,
}: {
  children: ReactNode;
  error?: boolean;
  tone?: "success" | "info" | "warning" | "error";
}) {
  const resolved = error ? "error" : (tone ?? "success");
  return (
    <div
      className={`notice ${resolved}`}
      role={resolved === "error" ? "alert" : "status"}
    >
      {children}
    </div>
  );
}
export function StatusBadge({
  status,
  label,
  tone,
}: {
  status: string;
  label?: string;
  tone?: BadgeTone;
}) {
  const t = useCopy();
  const meta = statusMeta(status, t);
  return (
    <span className={`badge ${tone ?? meta.tone}`}>{label ?? meta.label}</span>
  );
}
export function Empty({
  title,
  children,
  action,
  headingLevel = 2,
}: {
  title: string;
  children?: ReactNode;
  action?: ReactNode;
  headingLevel?: 2 | 3;
}) {
  return (
    <div className="empty">
      <div className="empty-icon">
        <Inbox size={26} />
      </div>
      {headingLevel === 3 ? <h3>{title}</h3> : <h2>{title}</h2>}
      {children ? <p className="muted">{children}</p> : null}
      {action}
    </div>
  );
}
export function Busy({ children }: { children: ReactNode }) {
  return (
    <span className="busy">
      <LoaderCircle size={16} className="spin" />
      {children}
    </span>
  );
}
export function Details({
  title,
  children,
  defaultOpen = false,
}: {
  title: string;
  children: ReactNode;
  defaultOpen?: boolean;
}) {
  return (
    <details className="details" open={defaultOpen || undefined}>
      <summary>{title}</summary>
      <div className="details-body">{children}</div>
    </details>
  );
}
export function Stat({
  label,
  value,
  detail,
}: {
  label: string;
  value: string | number;
  detail?: string;
}) {
  return (
    <div className="stat">
      <span className="muted">{label}</span>
      <strong>{value}</strong>
      {detail && <small>{detail}</small>}
    </div>
  );
}

export interface SubTabItem<T extends string = string> {
  value: T;
  label: string;
  icon?: ReactNode;
  content: ReactNode;
  keepMounted?: boolean;
}

export function SubTabs<T extends string>({
  value,
  onChange,
  label,
  items,
}: {
  value: T;
  onChange: (value: T) => void;
  label: string;
  items: SubTabItem<T>[];
}) {
  return (
    <Tabs.Root
      value={value}
      onValueChange={(next) => {
        if (typeof next === "string") onChange(next as T);
      }}
      className="subtabs-root"
    >
      <Tabs.List className="subtabs" aria-label={label}>
        {items.map((item) => (
          <Tabs.Tab key={item.value} value={item.value} className="subtab">
            {item.icon}
            <span>{item.label}</span>
          </Tabs.Tab>
        ))}
      </Tabs.List>
      {items.map((item) => (
        <Tabs.Panel
          key={item.value}
          value={item.value}
          keepMounted={item.keepMounted}
          className="subtab-panel"
        >
          {item.content}
        </Tabs.Panel>
      ))}
    </Tabs.Root>
  );
}

export function formatCountdown(totalSecs: number): string {
  const clamped = Math.max(0, Math.floor(totalSecs));
  const hours = Math.floor(clamped / 3600);
  const mins = Math.floor((clamped % 3600) / 60);
  const secs = clamped % 60;
  if (hours > 0) {
    return `${hours}:${String(mins).padStart(2, "0")}:${String(secs).padStart(2, "0")}`;
  }
  return `${mins}:${String(secs).padStart(2, "0")}`;
}

export function ArmLiveWritesModal({
  open,
  onOpenChange,
  liveWrites,
  expiresInSecs = null,
  timeoutSecs = 900,
  busy,
  runningJob,
  error,
  onConfirm,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  liveWrites: boolean;
  expiresInSecs?: number | null;
  timeoutSecs?: number;
  busy: boolean;
  runningJob?: boolean;
  error?: string;
  onConfirm: (enable: boolean, timeoutSecs?: number) => Promise<void>;
}) {
  const t = useCopy();
  const [checked, setChecked] = useState(false);
  const [duration, setDuration] = useState(String(timeoutSecs));

  function handleOpenChange(next: boolean) {
    if (!busy) {
      if (next) setDuration(String(timeoutSecs));
      if (!next) setChecked(false);
      onOpenChange(next);
    }
  }

  const durationOptions = [
    { value: "300", label: t("5 minutes", "5 Minuten") },
    { value: "900", label: t("15 minutes (recommended)", "15 Minuten (empfohlen)") },
    { value: "1800", label: t("30 minutes", "30 Minuten") },
    { value: "3600", label: t("60 minutes", "60 Minuten") },
    { value: "0", label: t("Until locked or quit", "Bis Sperre oder Beenden") },
  ];

  return (
    <Modal
      open={open}
      onOpenChange={handleOpenChange}
      title={
        liveWrites
          ? t("Manage live writes mode", "Live-Schreibzugriff verwalten")
          : t("Enable live provider writes?", "Live-Schreibzugriff aktivieren?")
      }
      description={
        liveWrites
          ? t(
              "Disarming immediately blocks new provider writes across all accounts. You can also extend the auto-disarm timer.",
              "Das Deaktivieren sperrt sofort neue Schreibzugriffe für alle Konten. Du kannst den Auto-Deaktivierungs-Timer auch verlängern.",
            )
          : t(
              "This lets Mailroom create folders, publish sorting rules, move or label existing mail, forward verified invoices, and send unsubscribe requests across your connected accounts. It automatically returns to read-only when the timer expires, when you lock the vault, or when you quit Mailroom.",
              "Damit darf Mailroom Ordner anlegen, Sortierregeln aktivieren, vorhandene Mails sortieren oder labeln, geprüfte Rechnungen weiterleiten und Abmeldungen senden. Der Modus kehrt automatisch zu Nur-Lese zurück, wenn der Timer abläuft, der Tresor gesperrt oder Mailroom beendet wird.",
            )
      }
    >
      <div className="page-stack">
        {liveWrites ? (
          <>
            {expiresInSecs !== null && (
              <>
                <div className="notice info">
                  <span>
                    {runningJob
                      ? t(
                          `Auto-disarms in ${formatCountdown(expiresInSecs)} (paused while a job is running).`,
                          `Automatische Deaktivierung in ${formatCountdown(expiresInSecs)} (pausiert während laufender Aktionen).`,
                        )
                      : t(
                          `Auto-disarms in ${formatCountdown(expiresInSecs)}.`,
                          `Automatische Deaktivierung in ${formatCountdown(expiresInSecs)}.`,
                        )}
                  </span>
                </div>
                <span className="sr-only" role="status" aria-live="polite">
                  {expiresInSecs === 60
                    ? t(
                        "One minute remaining before live writes automatically disarm.",
                        "Noch eine Minute bis zur automatischen Deaktivierung des Live-Schreibzugriffs.",
                      )
                    : ""}
                </span>
              </>
            )}
            {runningJob && (
              <Notice tone="warning">
                <ShieldAlert size={16} />
                {t(
                  "An operation is currently running. Returning to read-only prevents new writes, while an already-started operation must be stopped or allowed to settle.",
                  "Eine Aktion läuft derzeit. Der Nur-Lese-Modus verhindert neue Schreibzugriffe; eine bereits gestartete Aktion muss gestoppt oder abgewartet werden.",
                )}
              </Notice>
            )}
            {error && <Notice error>{error}</Notice>}
            <div className="dialog-actions">
              <Action onClick={() => handleOpenChange(false)} disabled={busy}>
                {t("Keep armed", "Aktiv lassen")}
              </Action>
              {timeoutSecs > 0 && (
                <Action
                  disabled={busy}
                  onClick={async () => {
                    await onConfirm(true, timeoutSecs);
                  }}
                >
                  {t(
                    `Reset timer (${Math.round(timeoutSecs / 60)}m)`,
                    `Timer zurücksetzen (${Math.round(timeoutSecs / 60)} Min.)`,
                  )}
                </Action>
              )}
              <Action
                primary
                disabled={busy}
                onClick={async () => {
                  await onConfirm(false);
                  setChecked(false);
                }}
              >
                <ShieldCheck size={15} />
                {t("Return to read-only", "Zurück zu Nur-Lese")}
              </Action>
            </div>
          </>
        ) : (
          <>
            <Choice
              label={t("Auto-disarm timer", "Auto-Deaktivierung")}
              fieldLabel={t(
                "Automatically return to read-only after",
                "Automatisch zurück zu Nur-Lese nach",
              )}
              value={
                durationOptions.some((o) => o.value === duration)
                  ? duration
                  : "900"
              }
              onChange={setDuration}
              options={durationOptions}
            />
            <CheckField checked={checked} onChange={setChecked}>
              {t(
                "I understand that confirmed actions can modify my connected mail accounts. Every write still requires its own preview and confirmation.",
                "Mir ist bewusst, dass bestätigte Aktionen meine verbundenen Mailkonten verändern können. Jede Änderung erfordert weiterhin eine eigene Vorschau und Bestätigung.",
              )}
            </CheckField>
            {error && <Notice error>{error}</Notice>}
            <div className="dialog-actions">
              <Action onClick={() => handleOpenChange(false)} disabled={busy}>
                {t("Stay read-only", "Im Nur-Lese-Modus bleiben")}
              </Action>
              <Action
                tone="warning"
                disabled={!checked || busy}
                onClick={async () => {
                  await onConfirm(true, Number(duration));
                  setChecked(false);
                }}
              >
                <ShieldAlert size={15} />
                {busy ? (
                  <Busy>{t("Enabling…", "Wird aktiviert…")}</Busy>
                ) : (
                  t("Enable live writes", "Live-Schreibzugriff aktivieren")
                )}
              </Action>
            </div>
          </>
        )}
      </div>
    </Modal>
  );
}
