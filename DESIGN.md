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

# Design

<!-- impeccable:design-map -->
Name: mailroom
Colors: primary: primary #147d70; neutral: background #f7f9f9; warning: warning #b45309; danger: danger #b91c1c; primary: primary-strong #106659; primary-soft #e6f3f0 (+12 more in the sidecar)
Fonts: Inter (body)
Spacing: compact 10px, group 16px, section 24px
Radius: control 8px, panel 12px, pill 999px
Don't:
- Don't use gradients, cream backgrounds, eyebrow kickers, italic heading accents, section numbers, or border-left side-tabs.
- Don't claim "Encrypted local vault" in synthetic sessions or enable unavailable actions.
Do:
- Do label synthetic demo sessions in the topbar, sidebar, footer, and unavailable panels.
- Do use plain user verbs over protocol acronyms and localize server errors in EN and DE.
- Do verify German UI at `390px` width.
Full: .impeccable/design-full.md (read a section when you need its detail)
