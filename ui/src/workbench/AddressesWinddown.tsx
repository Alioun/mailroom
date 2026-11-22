import { useState } from "react";
import { BookUser, Forward, TrendingDown } from "lucide-react";
import type { Account } from "../api";
import { SubTabs, useCopy } from "./components";
import type { SubTabItem } from "./components";
import { LedgerPanel } from "./winddown/LedgerPanel";
import { GmailForwardingPanel } from "./winddown/GmailForwardingPanel";
import { WinddownPanel } from "./winddown/WinddownPanel";

type Tab = "ledger" | "forwarding" | "winddown";

export function AddressesWinddown({ account }: { account: Account }) {
  const t = useCopy();
  const [tab, setTab] = useState<Tab>("ledger");

  const items: SubTabItem<Tab>[] = [
    {
      value: "ledger",
      label: t("Address ledger", "Adressverzeichnis"),
      icon: <BookUser size={15} />,
      content: <LedgerPanel account={account} />,
    },
    {
      value: "forwarding",
      label: t("Gmail forwarding", "Gmail-Weiterleitung"),
      icon: <Forward size={15} />,
      content: <GmailForwardingPanel />,
    },
    {
      value: "winddown",
      label: t("Account wind-down", "Konto-Abbau"),
      icon: <TrendingDown size={15} />,
      content: <WinddownPanel account={account} />,
    },
  ];

  return (
    <div className="page-stack">
      <SubTabs
        value={tab}
        onChange={setTab}
        label={t("Addresses & wind-down sections", "Bereiche für Adressen & Abbau")}
        items={items}
      />
    </div>
  );
}
