---
name: mailroom
description: A restrained local interface for inspecting mail decisions.
colors:
  primary: "#15596a"
  background: "#ffffff"
  foreground: "#203239"
  secondary-text: "#53656e"
  divider: "#ccd6da"
  sample-surface: "#eaf0f3"
typography:
  body:
    fontFamily: '-apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif'
    fontSize: "16px"
    lineHeight: 1.55
  heading:
    fontSize: "clamp(30px, 4vw, 43px)"
    fontWeight: 650
    lineHeight: 1.18
    letterSpacing: "-0.035em"
rounded:
  control: "5px"
  panel: "7px"
spacing:
  compact: "10px"
  group: "20px"
  section: "30px"
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.background}"
    rounded: "{rounded.control}"
    padding: "9px 16px"
---

## Overview

This is an Operate interface. The current screen helps a reader compare message destinations and understand why each rule matched. Keep synthetic-source labels and account restrictions visible.

## Colors

Use white for the page, blue-tinted gray for the sample, and deep teal for the primary action and filing destinations. Use secondary text for explanations. Borders separate decisions without surrounding each row with a card.

## Typography

Use the native sans-serif stack for interface text. Keep headings upright. Monospace is reserved for generated code and raw JSON. Use tabular numerals for counts.

## Layout

The page has a maximum width of 1200px with 40px side padding. Below 680px, use 20px padding, stack the sample controls, and show decisions as compact message rows. Keep copy actions separate from filing destinations.

## Elevation & Depth

Use borders and surface colors. There are no shadows.

## Shapes

Controls have small rounded corners. Avoid pill buttons and nested cards.

## Components

Ark UI toggle groups provide language and result selection. Selected controls use a tinted fill and border. The primary preview button has a dark fill with white text. Controls are at least 44px high. Keyboard focus has a visible outline. Disabled controls remain legible.

## Do's and Don'ts

- Do show source, decisions and unavailable operations in plain language.
- Do verify German text at narrow widths.
- Don't add gradients, cream backgrounds, italic heading accents or decorative section numbers.
- Don't show account actions as successful while their implementation is unavailable.
