---
name: create-yaac-theme
description: Create or revise a custom YAAC window theme. Use when the user asks for a YAAC color scheme, to adapt a named palette to YAAC, or to preview and apply a generated YAAC theme.
---

# Create a YAAC theme

Use the requested palette and the app's color roles to produce a theme that works in the chat, sidebar, and composer. Keep primary button text readable against the accent, secondary text readable on inputs and panels, and success, warning, and failure states distinguishable from one another.

## Theme format

Write a UTF-8 JSON object up to 16 KiB. Give it a distinct, case-sensitive name of 1–64 UTF-8 bytes. System, Dark, Light, and Custom are reserved in any capitalization. Select `"base": "dark"` or `"base": "light"` to inherit unspecified colors from the corresponding YAAC palette. Use opaque six-digit hex colors.

```json
{
  "name": "Harbor",
  "base": "dark",
  "colors": {
    "bg": "#131F2B",
    "panel": "#192B3A",
    "faint": "#223143",
    "text": "#E6EEF5",
    "text2": "#ADBDC9",
    "primary": "#8ADACE",
    "on_primary": "#102923",
    "user_bg": "#254051",
    "done": "#74D69A"
  }
}
```

Supported keys by role:

- Surfaces: `bg` (window), `panel` (sidebar), `raised` (cards), `faint` (inputs and code), `user_bg` (user messages), `border`.
- Text and controls: `text`, `text2` (secondary), `primary` (accent), `on_primary` (text on accent), `selected`.
- Status: `needs` (warning), `done` (success), `failed`, `fast`, `ultrafast`, `shell`.

## Preview and apply

1. Use `yaac.list_themes` to find the current theme and check for a name conflict. For a new design, choose a distinct name. Use `overwrite: true` when the user explicitly asks to replace the existing theme.
2. Call `yaac.import_theme` with the JSON string, or with the relative path to a JSON file in the thread's worktree. This adds the theme to the YAAC window's library.
3. Call `yaac.preview_theme` with the exact name. The preview appears as a PNG in the chat and preserves the active theme.
4. Call `yaac.select_theme` when the user asks to apply or switch to the theme. For a request to create or preview it, show the result and name without changing the active theme.

If the window is unavailable, return the JSON or present the generated file when the user requested a downloadable copy.
