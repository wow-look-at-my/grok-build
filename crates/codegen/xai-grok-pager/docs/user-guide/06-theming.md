# Theming and Appearance Customization

Grok Build draws all TUI colors from a central theme. You can switch themes while Grok is running, follow your operating system's light or dark appearance, and adjust scrollback layout, animations, and block styling through configuration files.

---

## Available Themes

Grok includes six built-in themes, plus an `auto` option that follows your system appearance:

| Theme | Config Names | Description | Truecolor Required |
|-------|-------------|-------------|--------------------|
| **GrokNight** | `groknight`, `grok-night`, `dark` | Neutral dark base with a magenta accent. Default theme. Survives quantization cleanly on 256-color and 16-color terminals. | No |
| **GrokDay** | `grokday`, `grok-day`, `light`, `day` | Light theme for bright terminal backgrounds. | No |
| **TokyoNight** | `tokyonight`, `tokyo-night`, `tokyo` | Dark, blue-tinted backgrounds from the Tokyo Night palette. Loses its character when quantized. | Yes |
| **RosePineMoon** | `rosepine`, `rose-pine`, `rosepine-moon`, `rose-pine-moon` | Muted dark palette with mauve accents, from the Rosé Pine family. | Yes |
| **OscuraMidnight** | `oscura`, `oscura-midnight` | Deep dark base with purple accents. | Yes |
| **Terminal** | `terminal`, `terminal-default`, `transparent`, `native` | Your terminal's own colors: no background of its own, so the terminal canvas (translucency, background images) shows through. | No |

Theme names are case-insensitive. The `auto` option (alias `system`) is documented under [Auto Theme (System Appearance)](#auto-theme-system-appearance).

### Terminal Theme

`terminal` paints no surface backgrounds and defines almost no colors of its own — everything comes from your terminal profile. The scrollback, composer, modals, and status line leave the terminal's canvas visible (a translucent or image-backed window shows through Grok the way it shows through your shell), body text uses the terminal's default foreground, and accents (errors, diffs, links, syntax) come from your profile's 16-color ANSI palette. Because it borrows your profile's colors instead of assuming a light or dark background, it stays readable on any profile with no appearance detection, and it renders identically at every color depth.

The theme paints no backgrounds at all: your messages render in bold instead of on a band, menus and prompt panels sit directly on the terminal canvas, and the selected or hovered row in any menu uses reverse video (your terminal's own foreground/background swap), so every combination stays readable on any profile. Decoration — idle borders, dividers, the scrollbar thumb — uses ANSI *bright black* as a foreground, the palette slot your profile tunes as its own dimmed tone. Focused chrome, like the active composer border, stays at the full default foreground so focus still pops. Grok also leaves your cursor color alone on this theme (other themes recolor it to their accent).

```json
{
  "ui": {
    "theme": "terminal"
  }
}
```

Contrast is only as good as your terminal profile: a profile with a very dark bright-black slot will render faint dividers, since Grok derives everything from your palette rather than hard-coding colors.

The theme rolls out gradually. Until the rollout reaches your account it is hidden from `/theme` and `/settings`, its names do not parse. A configured `"theme": "terminal"` falls back to the default theme. Set `GROK_TERMINAL_THEME=1` (or `"terminal_theme": true` under `features` in `config.json`) to enable it locally ahead of the rollout.

---

## Switching Themes

### In the TUI

Run the `/theme` slash command (alias `/t`) to open the theme picker. As you move through the list with the arrow keys, Grok previews each theme in real time. Press Enter to apply and save your choice, or press Escape to revert. Typing filters the list by any of a theme's config names, so `/theme transparent` narrows it to the Terminal row.

To switch without the picker, pass a name directly:

```
/theme tokyonight
```

Submitting `/theme` on its own -- without choosing from the picker -- cycles to the next theme.

### Via Config File

Set the theme in `~/.grok/config.json`:

```json
{
  "ui": {
    "theme": "tokyonight"
  }
}
```

---

## Auto Theme (System Appearance)

Set `theme = "auto"` to have Grok follow your operating system's light/dark appearance and switch themes automatically:

```json
{
  "ui": {
    "theme": "auto"
  }
}
```

By default, dark mode maps to **GrokNight** and light mode maps to **GrokDay**. Override either mapping with `auto_dark_theme` and `auto_light_theme`:

```json
{
  "ui": {
    "auto_dark_theme": "tokyonight",
    "auto_light_theme": "grokday",
    "theme": "auto"
  }
}
```

`theme = "system"` is an alias for `theme = "auto"`.

### How Detection Works

| Platform | Method |
|----------|--------|
| **macOS** | Reads `AppleInterfaceStyle` system preference |
| **Linux** | Queries XDG Desktop Portal (`org.freedesktop.appearance.color-scheme`) |
| **Windows** | Reads the system personalization registry |
| **SSH / tmux / headless** | `GROK_APPEARANCE` or `LC_GROK_APPEARANCE` (`dark`/`light`), then `COLORFGBG`, then a startup OSC 11 background query. `grok wrap ssh …` stamps `LC_GROK_APPEARANCE` from the local OS theme so it survives SSH into the login shell. New tmux sessions inherit it only if the tmux server/session was created with that env (or `update-environment` includes it). OSC 11 is DCS-wrapped for tmux ≥ 3.3 when tmux is the immediate terminal (not an editor `:terminal`); reaching the outer emulator also needs `allow-passthrough`, and replies are best-effort. |

Once running, Grok polls desktop APIs and env hints every 5 seconds. Toggling your OS between light and dark mode on a local desktop takes effect within seconds without restarting. Over SSH the wrap-stamped env is fixed for that hop.

You can also set `GROK_THEME` (or `LC_GROK_THEME`) to force a theme or `auto` without editing `config.json`.

### Via the Settings Pane

Run `/settings` (alias `/config`) and open the **Appearance** category to set the **Auto dark theme** and **Auto light theme** interactively. Selecting `auto` in the `/theme` picker enables auto mode using these mappings.

---

## Color Support Detection

On startup, Grok detects your terminal's color capability level:

| Level | Description | Detection |
|-------|-------------|-----------|
| **Truecolor** (24-bit) | Full RGB color. All themes render as designed. | `COLORTERM=truecolor` or equivalent terminal capability |
| **256-color** | Indexed palette. RGB values are mapped to the nearest palette entry. | Standard xterm-256color |
| **16-color** | ANSI names only. Colors are mapped to the closest ANSI color. | Basic terminal support |

When you set `NO_COLOR`, Grok emits no color and renders in monochrome.

Run `/doctor` to see the detected color level and the themes available on this terminal. If truecolor is unavailable, Doctor shows the relevant setup steps or explains the terminal limitation.

### Automatic Quantization

Every theme is defined using full RGB values. At startup, Grok quantizes all colors to match the detected capability level. This means:

- On **truecolor** terminals, colors pass through unchanged.
- On **256-color** terminals, each RGB value is mapped to the nearest indexed palette entry.
- On **16-color** terminals, colors map to ANSI names.

GrokNight and GrokDay use neutral grays that quantize cleanly. TokyoNight, RosePineMoon, and OscuraMidnight use distinctive tinted backgrounds that lose their character when quantized, which is why the theme picker hides them on non-truecolor terminals.

### Runtime-Generated Colors

Colors generated at runtime (syntax highlighting, background blending) are also quantized through the same pipeline, ensuring consistent appearance across all terminal types.

---

## Cursor Color

Grok sets your terminal cursor to the current theme's `accent_user` color using the OSC 12 escape sequence, to indicate an active Grok session. The cursor color is:

- Applied on startup and on theme switch.
- Reset to the terminal's default on exit via OSC 112.

This works in terminals that support OSC 12 (most modern terminals).

---

## Compact Mode

Toggle compact mode with the `/compact-mode` slash command. Compact mode:

- Removes outer vertical padding (top/bottom margins become 0).
- Reduces horizontal padding to the minimum (1 column).
- Reduces top padding in the prompt area and info blocks.

The setting is persisted in `~/.grok/config.json` under `ui.compact_mode` and survives restarts.

Use compact mode on small screens to maximize content area.

---

## Syntax Highlighting

Grok bundles three `.tmTheme` files for code-block syntax highlighting and selects one based on the active theme:

- `grok-night.tmTheme` -- GrokNight, RosePineMoon, and OscuraMidnight
- `grok-day.tmTheme` -- GrokDay
- `tokyo-night.tmTheme` -- TokyoNight

Grok selects the matching file automatically when you switch themes. The `.tmTheme` files are built into the binary, so you cannot replace them with your own.

---

## Deep Customization with pager.json

For fine-grained control over the TUI appearance, edit `~/.grok/pager.json`. This file controls scrollback layout, block styling, animations, and more. All settings have defaults. Specify only the values you override. An absent or `null` member keeps the default.

grok writes `~/.grok/pager.schema.json` at startup and creates `pager.json` with only its `"$schema"` member when the file does not exist. An editor that reads JSON Schema then completes every key and shows its default and its description. A key that you leave out follows the default of each new release. A legacy `pager.toml` converts to `pager.json` at startup (see [Configuration](05-configuration.md#pagerjson-appearance-configuration)).

The blocks below are parts of one file. A whole file looks like this:

```json
{
  "$schema": "./pager.schema.json",
  "animation": {
    "fps": 60
  },
  "scrollback": {
    "blocks": {
      "tool": {
        "bullet": "small-triangle"
      }
    }
  }
}
```

### Layout

Control viewport padding and block spacing. The left and right margins are at least 1. `block_pad_left` is the padding between the accent line and the content.

```json
{
  "scrollback": {
    "layout": {
      "outer_vpad": 1,
      "outer_hpad_left": 2,
      "outer_hpad_right": 2,
      "block_pad_left": 2,
      "block_pad_right": 2
    }
  }
}
```

### Scrollbar

A gap of 0 puts the scrollbar next to the content or at the screen edge.

```json
{
  "scrollback": {
    "scrollbar": {
      "enabled": true,
      "gap_left": 0,
      "gap_right": 0,
      "scrollbar_bg": "none",
      "scrollbar_fg": "none"
    }
  }
}
```

### Scroll Behavior

```json
{
  "scrollback": {
    "scroll": {
      "margin": 0,
      "min_page_fraction": 0,
      "follow_indicator": "center",
      "follow_auto_select": true,
      "follow_by_overscroll": true,
      "anchor_on_fold": true
    }
  }
}
```

### Display Options

`tab_width` no passes tabs through. `collapsed_accent_char` falls back to `"|"` on the legacy Windows console. `dim_accent` is a blend factor from 0.0 to 1.0.

```json
{
  "scrollback": {
    "display": {
      "sticky_headers": true,
      "tab_width": 4,
      "expandable_indicator": true,
      "expandable_indicator_char": "›",
      "collapsed_accent_char": "❙",
      "dim_accent": 0.5,
      "line_under_last_entry": false,
      "selection_buttons": false
    }
  }
}
```

### Animation

`fps` is the frame rate, from 1 to 60. A higher rate is smoother and uses more CPU.

```json
{
  "animation": {
    "fps": 30,
    "wave_rows": 32
  }
}
```

### Block Styling: Edit Diffs

`expanded_by_default` unset (`null`) follows the Collapsed edit blocks setting. When that setting is on, edits start collapsed even when this key is `true`. `line_summary` shows +N/-M in the collapsed header, and unset follows the same setting.

```json
{
  "scrollback": {
    "blocks": {
      "edit": {
        "indent": true,
        "vpad": false,
        "expanded_by_default": null,
        "hunk_separator": "…",
        "dual_line_numbers": false,
        "line_summary": null,
        "bg": "none"
      }
    }
  }
}
```

### Block Styling: Thinking/Reasoning

`bg_blend` blends the markdown colors with the background (0-100). `header` shows the "Thinking..." header, and `header_bright` gives it the bright style.

```json
{
  "scrollback": {
    "blocks": {
      "thinking": {
        "accent_enabled": true,
        "animate": true,
        "truncated_lines": 3,
        "bg_blend": 70,
        "header": true,
        "header_bright": false
      }
    }
  }
}
```

### Block Styling: Tool Calls

`muted_collapsed` grays out collapsed tool calls. `dim_details` dims the details in parentheses, such as line counts and match counts.

```json
{
  "scrollback": {
    "blocks": {
      "tool": {
        "muted_collapsed": true,
        "dim_details": true,
        "bullet": "diamond"
      }
    }
  }
}
```

Available bullet styles:

| Config Value | Character | Description |
|-------------|-----------|-------------|
| `none` | (none) | No bullet |
| `dot` | `·` | Middle dot (smallest) |
| `small-circle` | `•` | Bullet |
| `circle` | `●` | Filled circle |
| `small-triangle` | `▸` | Right-pointing small triangle |
| `triangle` | `▶` | Right-pointing triangle |
| `diamond` | `◆` | Filled diamond (default) |

### Block Styling: Execute (Shell Commands)

`first_lines` and `last_lines` are the output lines shown at the start and at the end in truncated mode. `header_style` is `"shell"` for a `$` prefix or `"label"` for a Run prefix.

```json
{
  "scrollback": {
    "blocks": {
      "execute": {
        "first_lines": 2,
        "last_lines": 3,
        "accent_enabled": true,
        "header_style": "label",
        "muted_command_collapsed": true
      }
    }
  }
}
```

### Block Styling: User Prompts (Scrollback)

`min_lines` is the minimum content lines in truncated and sticky mode.

```json
{
  "scrollback": {
    "blocks": {
      "prompt": {
        "vpad": true,
        "bg": "light",
        "show_prefix": true,
        "min_lines": 2
      }
    }
  }
}
```

### Prompt Input Widget

```json
{
  "prompt": {
    "collapse_unfocused": true,
    "mouse_hover": true,
    "show_prefix": true
  }
}
```

### Terminal Behavior

```json
{
  "terminal": {
    "alt_screen": "auto"
  }
}
```

Alt-screen policies:
- `auto` -- fullscreen in plain terminals and normal tmux; inline in tmux control mode and Zellij.
- `always` -- always enter fullscreen.
- `never` -- never enter fullscreen; run inline in the main scrollback.

### Plugins UI

```json
{
  "disable_plugins": false
}
```

---

## Theme Color Slots

Each theme defines the following color slots that are used throughout the TUI:

**Backgrounds:** `bg_base`, `bg_light`, `bg_dark`, `bg_highlight`, `bg_hover`, `bg_terminal`, `bg_visual`

**Accents:** `accent_user`, `accent_assistant`, `accent_thinking`, `accent_tool`, `accent_system`, `accent_error`, `accent_success`, `accent_running`, `accent_skill`, `accent_plan`, `accent_verify`, `accent_remember`, `accent_model`

**Text:** `text_primary`, `text_secondary`

**Grays:** `gray_dim`, `gray`, `gray_bright`

**Semantic:** `command`, `path`, `running`, `warning`, `fuzzy_accent`

**Borders and scrollbar:** `selection_border`, `hover_border`, `prompt_border`, `prompt_border_active`, `scrollbar_bg`, `scrollbar_fg`

**Paste:** `paste_bg`, `paste_fg`, `paste_dim`

**Diff:** `diff_delete_bg`, `diff_delete_fg`, `diff_insert_bg`, `diff_insert_fg`, `diff_equal_fg`, `diff_gutter_fg`

**Markdown:** heading colors (`md_heading_h1`-`md_heading_h6`), `md_code`, `md_code_bg`, `md_text`, `md_muted`, `md_task_checked`, `md_task_unchecked`, `link_fg`

The theme system manages these slots internally and quantizes them automatically for your terminal.
