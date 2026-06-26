# LLM Wiki Design System

## 1. Atmosphere & Identity

LLM Wiki feels like a quiet desktop knowledge workbench: dense, legible, and calm. The signature is a neutral split-pane workspace where borders, muted surfaces, and compact controls keep attention on documents, graphs, and chat output rather than decoration.

## 2. Color

### Palette

| Role | Token | Light | Dark | Usage |
|------|-------|-------|------|-------|
| Surface/base | `--background` | `oklch(1 0 0)` | `oklch(0.16 0.005 260)` | App background |
| Surface/card | `--card` | `oklch(1 0 0)` | `oklch(0.205 0.005 260)` | Panels and cards |
| Surface/muted | `--muted` | `oklch(0.97 0 0)` | `oklch(0.269 0.005 260)` | Empty states and subtle rows |
| Text/primary | `--foreground` | `oklch(0.145 0 0)` | `oklch(0.985 0 0)` | Body and headings |
| Text/secondary | `--muted-foreground` | `oklch(0.556 0 0)` | `oklch(0.708 0 0)` | Captions and metadata |
| Border/default | `--border` | `oklch(0.922 0 0)` | `oklch(1 0 0 / 12%)` | Dividers and outlines |
| Accent/default | `--accent` | `oklch(0.97 0 0)` | `oklch(0.269 0.005 260)` | Hover rows and secondary controls |
| Status/error | `--destructive` | `oklch(0.577 0.245 27.325)` | `oklch(0.704 0.191 22.216)` | Destructive actions and errors |

### Rules

- Use semantic Tailwind tokens such as `bg-background`, `text-muted-foreground`, `border-border`, and `hover:bg-accent`.
- Keep color mostly neutral. Status colors are functional, never decorative.
- New raw colors must first be added here with a semantic role.

## 3. Typography

### Scale

| Level | Size | Weight | Line Height | Tracking | Usage |
|-------|------|--------|-------------|----------|-------|
| H2 | `text-sm` to `text-base` | 600 | normal | 0 | Panel headers |
| Body | `text-sm` | 400 | normal | 0 | Lists, controls, metadata-heavy UI |
| Caption | `text-xs` | 400-500 | relaxed when multiline | 0 | Hints, counts, tooltips |
| Micro | `text-[10px]` to `text-[11px]` | 400-500 | normal | 0 | Dense counters and lazy-load notes |

### Font Stack

- Primary: `Geist Variable`, sans-serif via `--font-sans`.
- Mono: system monospace for code blocks and technical snippets.

### Rules

- Operational UI stays compact and scannable.
- Avoid hero-scale text inside the app shell.

## 4. Spacing & Layout

### Base Unit

All spacing follows Tailwind's 4px scale.

| Token | Value | Usage |
|-------|-------|-------|
| `gap-1`, `p-1` | 4px | Icon row density |
| `gap-2`, `px-2`, `py-2` | 8px | Default toolbar/list rhythm |
| `px-4`, `py-3` | 16px / 12px | Panel header padding |
| `p-8` | 32px | Empty states |

### Grid

- The app is a resizable pane layout with stable overflow boundaries.
- Lists use truncation and virtualization/lazy loading where needed.

### Rules

- Prefer existing shadcn components and utility classes.
- Preserve scroll containment inside panels.

## 5. Components

### Source Tree Row

- **Structure**: icon, truncating filename/folder label, compact action buttons.
- **Variants**: folder, file, grouped root.
- **Spacing**: `gap-1`, `px-1`, `py-1`; nested rows add 16px indentation per depth.
- **States**: hover uses `hover:bg-accent`; disabled actions use the Button disabled state.
- **Accessibility**: folder toggles and file opens are buttons; destructive actions use two-step confirmation.

## 6. Motion & Interaction

### Timing

| Type | Duration | Easing | Usage |
|------|----------|--------|-------|
| Micro | 100-150ms | ease-out | Hover and pressed states |
| Standard | 200-300ms | ease-in-out | Panel and list updates |

### Rules

- Use transform/opacity for new motion.
- Keep long-running states explicit with disabled buttons or spinners.

## 7. Depth & Surface

### Strategy

Borders-only with subtle tonal hover states.

| Type | Value | Usage |
|------|-------|-------|
| Default | `border-border` | Panel edges and section dividers |
| Subtle | `bg-accent` / `bg-muted` | Hover and quiet group separation |

