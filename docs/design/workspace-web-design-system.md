# Workspace web design system

This document defines the visual rules for `web/workspace`. The current authority for tokens and reusable page/sidebar styling is `web/workspace/src/app.css`.

## Design position

Workspace web should read as a control surface, not a set of detached widgets. Group information primarily through spacing, typography, and text contrast. Use borders, rounded rectangles, shadows, and filled panels only when they clarify hierarchy that spacing cannot express.

## Palette

Colors are defined as CSS custom properties in OKLCH. The palette supports light and dark modes through `prefers-color-scheme`.

Rules:

- Background and layout surfaces use zero chroma: `oklch(... 0 0)`.
- Primary text and code text are near-neutral warm colors. Because CSS OKLCH exposes chroma (`C`) rather than saturation directly, encode the “about 5% saturation” intent as very low warm chroma, around `C = 0.01` to `0.012`.
- Muted text reduces lightness/chroma before introducing new hues.
- Accent/status colors are semantic exceptions. They should mark state, focus, or navigation, not decorate containers.
- Do not introduce raw hex/rgb colors in Workspace web components. Add or reuse a token in `app.css`.

Core tokens:

```css
--bg
--bg-raised
--bg-subtle
--line
--line-strong
--text
--text-strong
--text-muted
--text-faint
--code
--accent
--success
--warning
--danger
```

## Layout and grouping

Prefer vertical rhythm and text hierarchy over card chrome.

- Page sections are separated by whitespace and a light top rule.
- Navigation selection uses a left rule rather than filled pills.
- Nested records use indentation or top rules, not repeated rounded containers.
- Shadows are avoided in the base system.
- Rounded corners are reserved for small controls where hit area shape matters.

## Typography

- Headings and primary labels use `--text-strong`.
- Body text uses `--text`.
- Metadata, helper text, timestamps, and table headings use `--text-muted` or `--text-faint`.
- Uppercase labels are acceptable for small metadata labels only; avoid large all-caps UI blocks.

## Component styling boundary

`app.css` owns the shared visual language:

- reset/base body styles
- OKLCH tokens
- layout primitives
- page cards/sections
- sidebar/navigation sections
- tables, kanban lists, diagnostics, record bodies

Svelte components should keep local styles only when a behavior is truly component-specific. If a style affects color, spacing, borders, text hierarchy, or repeated record layout, it belongs in `app.css`.

## Adding new UI

When adding Workspace web UI:

1. Start with semantic HTML and existing classes from `app.css`.
2. Use spacing and text contrast first.
3. Use a border only when the boundary carries meaning.
4. Use background fills only for page-level surfaces or read-only code/record bodies.
5. If a new color is needed, define it as an OKLCH token and document why the existing semantic tokens are insufficient.
