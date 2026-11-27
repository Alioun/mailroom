---
name: mailroom
description: Local-first Operate workbench for safe mail inspection, rules, and sorting.
colors:
  primary: "#147d70"
  primary-strong: "#106659"
  primary-soft: "#e6f3f0"
  background: "#f7f9f9"
  surface: "#ffffff"
  foreground: "#263c3a"
  secondary-text: "#526663"
  divider: "#d9e3e0"
  control-border: "#8fa39d"
  warning: "#b45309"
  warning-surface: "#fffbeb"
  danger: "#b91c1c"
  danger-surface: "#fef2f2"
  dark-background: "#0d1413"
  dark-surface: "#141f1e"
  dark-foreground: "#e6f0ee"
  dark-secondary-text: "#a1b8b3"
  dark-divider: "#263a37"
typography:
  body:
    fontFamily: 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif'
    fontSize: "14px"
    lineHeight: 1.5
  heading:
    fontSize: "30px"
    fontWeight: 650
    lineHeight: 1.25
    letterSpacing: "-0.03em"
rounded:
  control: "8px"
  panel: "12px"
  pill: "999px"
spacing:
  compact: "10px"
  group: "16px"
  section: "24px"
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "#ffffff"
    rounded: "{rounded.control}"
    padding: "9px 16px"
---

## Overview

Mailroom is a local-first **Operate** workbench built with React and Base UI. Users connect personal mail accounts (stored in an encrypted local vault) or explore a clearly labelled synthetic demo account, inspect deduplicated header findings, author and simulate local rules, and only modify real mail after explicitly arming session-scoped Live writes plus per-action preview and confirmation.

## Colors & Themes

- **Light mode (default):** cool off-white canvas (`#f7f9f9`), crisp white panels (`#ffffff`), deep pine-slate ink (`#263c3a`), muted slate secondary copy (`#526663`), and deep teal primary accent (`#147d70` / `#106659`).
- **Dark mode (`data-theme="dark"`):** deep slate-pine canvas (`#0d1413`), elevated surfaces (`#141f1e`), high-contrast off-white ink (`#e6f0ee`), and WCAG 2.1 AA contrast across controls, badges, and notices.
- **Safety semantics:** Read-only uses calm teal (`#eef7f3` / `#175c4b`). Armed Live writes shifts the topbar badge to high-contrast amber (`#b45309` / `#ffffff`) and subtly tints the topbar so write capability is unmistakable at a glance.
- **Browser surfaces:** Theme `::selection`, `caret-color`, `text-underline-offset`, and `font-variant-numeric: tabular-nums` from the palette.

## Typography

- Use `Inter` with system UI fallbacks (`-apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif`) at `14px` body size (`1.5` line-height).
- Keep headings upright with `-0.02em` to `-0.03em` tracking; do not place eyebrow kickers above page headings.
- Reserve monospace for generated Sieve scripts, YAML documents, receipts, and file paths.
- Format all counts, percentages, and dates using the active UI locale (`en-US` or `de-DE`) with tabular numerals (`tabular-nums`) and proper singular/plural grammar.

## Layout & Navigation

- Persistent `234px` left sidebar on desktop (collapsing into a Base UI focus-trapped `NavigationDrawer` below `768px`) with two clear groups (`Mailbox` and `Organize`) plus `Settings`.
- Sticky `70px` topbar carrying breadcrumb orientation, an explicit `Demo mail` pill whenever `status.synthetic` is active, the interactive Read-only / Live writes safety badge, Lock vault (when in vault mode), Theme switcher, Keyboard shortcuts trigger (`?`), and Language selector (`English` / `Deutsch`).
- Content column uses generous padding (`35px 38px` desktop, `20px 16px` mobile below `768px`, max-width `1510px`). Below `767px`, toolbars and filter selects stack cleanly at full width so German labels never collapse or clip.
- Long multi-section pages such as **Settings** begin with a compact in-page section jump strip, and destructive account actions (`Account key & local removal`) live at the bottom.

## Elevation, Surfaces & Shapes

- Primary elevation comes from `1px` borders (`--border`) over the canvas; reserve soft shadows strictly for floating overlays (dialogs, dropdown popups, mobile drawer).
- Standardize radii: `8px` for controls and status banners, `12px` for panels and dialogs, `999px` for small status pills and filter chips.
- Never use one-sided `border-left` accent bars ("side tabs") on rounded boxes; communicate notice/warning tone through a full tinted border, surface tint, and leading icon.

## Components & Accessibility

- Base UI (`Dialog`, `Select`, `Checkbox`, `Tabs`, `Button`) supplies accessible focus management, keyboard navigation, and Escape dismissal.
- Keep interactive touch targets usable (`>= 36–44px`), with distinct disabled surfaces rather than low-contrast opacity washout.
- Never place a 1-second ticking countdown inside `role="status"` or an `aria-live` region; announce safety state changes and threshold warnings cleanly without spamming screen readers.
- Keyboard shortcuts: `?` opens the shortcut reference, `/` focuses Senders search, `g` then `o`/`s`/`r`/`b`/`a` jumps between core views, and `Esc` closes open dialogs.

## Do's and Don'ts

- **Do** label synthetic demo sessions in the topbar, sidebar, footer, and unavailable panels.
- **Do** use plain user verbs over protocol acronyms and localize server errors in EN and DE.
- **Do** verify German UI at `390px` width.
- **Don't** use gradients, cream backgrounds, eyebrow kickers, italic heading accents, section numbers, or border-left side-tabs.
- **Don't** claim "Encrypted local vault" in synthetic sessions or enable unavailable actions.
