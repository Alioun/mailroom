import { useEffect, useState } from "react";
import type { ReactNode, RefObject } from "react";
import { Dialog } from "@base-ui/react/dialog";
import { X } from "lucide-react";
import { useCopy } from "./components";

export function NavigationDrawer({
  open,
  onOpenChange,
  triggerRef,
  headingRef,
  shouldFocusHeadingRef,
  children,
}: {
  open: boolean;
  onOpenChange: (value: boolean) => void;
  triggerRef: RefObject<HTMLButtonElement | null>;
  headingRef?: RefObject<HTMLHeadingElement | null>;
  shouldFocusHeadingRef?: RefObject<boolean>;
  children: ReactNode;
}) {
  const t = useCopy();
  const [mobile, setMobile] = useState(
    () => matchMedia("(max-width: 767px)").matches,
  );
  useEffect(() => {
    const query = matchMedia("(max-width: 767px)");
    const update = () => {
      setMobile(query.matches);
      onOpenChange(false);
    };
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, [onOpenChange]);
  if (!mobile) return <aside className="sidebar">{children}</aside>;
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Backdrop className="navigation-backdrop" />
        <Dialog.Popup
          className="sidebar mobile-drawer"
          finalFocus={() => {
            if (shouldFocusHeadingRef?.current && headingRef?.current) {
              shouldFocusHeadingRef.current = false;
              return headingRef.current;
            }
            return triggerRef.current ?? true;
          }}
        >
          <Dialog.Title className="sr-only">
            {t("Navigation", "Navigation")}
          </Dialog.Title>
          <Dialog.Close
            className="icon-button drawer-close"
            aria-label={t("Close navigation", "Navigation schließen")}
          >
            <X size={20} />
          </Dialog.Close>
          {children}
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
