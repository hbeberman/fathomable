---
type: Decision
title: Theme schema
description: The KDL theme file format, its key vocabulary, colour values, palette, inheritance, and how a theme is selected.
resource: crates/fathomable-core/src/theme.rs
related_resources:
  - crates/fathomable-core/themes/default-dark.kdl
  - crates/fathomable-core/themes/default-light.kdl
tags:
  - decision
  - configuration
  - rendering
---

# 0011 Theme schema

Status: accepted (2026-08-26); amended 2026-09-03 by
[0047](0047-one-vocabulary.md): the `annotation.*` keys are `thread.*`;
the old names still load for one release and `--doctor` names each one
a theme file sets. Amended 2026-09-04 by
[0049](0049-inline-threads-and-the-rail.md): `ui.sidebar*` is `ui.rail*`
on the same terms. Amended 2026-09-04 by
[0051](0051-retire-one-release-compatibility.md): both old spellings are
unknown keys now, and `--doctor` no longer reports them. Amended
2026-09-05 by [0057](0057-the-sidebar.md): `ui.rail*` is `ui.sidebar*`
again, and `ui.rail*` is an unknown key. Amended 2026-09-05 by
[0059](0059-headers-and-the-key-bar.md): `ui.header` is added. Amended
2026-09-05 by [0067](0067-the-texts-key-bar.md): `ui.hint` retires, an
unknown key now. Amended 2026-09-14 by
[0078](0078-all-keys-stays-reachable.md): help filtering uses the picker
match and selection roles; the built-ins give help, pickers, key menus, and
right-click menus the same quiet overlay treatment while keeping `ui.popup`
and `ui.menu` independently customizable.
Amended 2026-09-14 by [0079](0079-list-focus-language.md): four shared
`ui.list.*` roles replace `ui.sidebar.selected` and `ui.picker.selected`;
active, remembered, and hover treatments are distinct from neutral headers.

## Context

[0004](0004-markdown-rendering.md) and [0008](0008-configuration-format.md)
say themes are KDL files under `$XDG_CONFIG_HOME/fathomable/themes/`, and
[0010](0010-viewer-ux.md) names the keys the viewer needs, but no schema was
decided; the viewer shipped a hardcoded palette. Git gutter colours,
selection, and annotations all need theme keys, and the colour-blind use case
from 0010 needs users to be able to retune a palette in one place. The format
was settled in a question round on 2026-08-26, taking the user's Kyber theme
files and Helix's theme format as reference points.

## Decision

### File shape

A theme is one KDL document with four optional top-level nodes:

```kdl
inherits "default-dark"

palette {
    red    "#d54e53"
    yellow "#ffda03"
}

code {
    syntect "base16-ocean.dark"
}

colors {
    "ui.search.match"      fg="black" bg="yellow"
    "ui.statusline.normal" fg="black" bg="#7aa6da" mods="bold"
    "diff.plus"            "red"
    "markup.heading"       fg="#c397d8" mods="bold"
}
```

- `colors` holds one node per **dotted key** (quoted, since KDL identifiers
  cannot contain dots unquoted). A key takes `fg`, `bg`, and `mods`
  properties; a single positional string is shorthand for `fg`. `mods` is a
  space-separated list from `bold`, `dim`, `italic`, `underline`,
  `reversed`, `strikethrough`.
- `palette` maps names to colour values. A palette name may be used anywhere
  a colour is expected. Palette names shadow the ANSI names below.
- `inherits` names one parent theme. Keys not set in this file come from the
  parent; the parent is resolved by the same lookup as `--theme`. The
  built-in `default-dark` is the implicit root, so every key always has a
  value. Cycles are errors.
- `code.syntect` names one of syntect's bundled themes for code blocks.
  Loading `.tmTheme` files is parked.

### Colour values

A colour is one of: `#rrggbb`; a palette name; one of the sixteen ANSI names
(`black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`, and
their `bright-` variants), which use the terminal's own palette; or
`default`, which leaves that channel unset so the terminal background shows
through (0004's transparent dark theme). True colour is the design target; a
16-colour fallback remains parked.

### Key vocabulary

Markdown faces use Helix's `markup.*` names so Helix theme authors feel at
home; chrome uses `ui.*` and the gutter `diff.*` from 0010.

| Key | Use |
| --- | --- |
| `ui.text` | body text |
| `ui.linenr` | gutter line numbers |
| `ui.selection` | mouse or `V` selection; `ui.cursorline`, the cursor row, was removed on 2026-09-04 ([0010](0010-viewer-ux.md)) |
| `ui.search.match` | search highlights |
| `ui.statusline` | status line background |
| `ui.statusline.normal`, `.select`, `.input` | mode pills |
| `ui.statusline.info` | transient messages, pending keys, `[+]`, and muted ancestor-file context bars in review (0079) |
| `ui.warning` | the `deleted` banner over a file that is gone ([0028](0028-live-workspace.md)) |
| `diff.plus`, `diff.delta`, `diff.minus` | gutter diff bar |
| `git.staged`, `git.unstaged` | the tree pane's git letters ([0017](0017-git-status-navigation.md)) |
| `ui.sidebar`, `ui.sidebar.dir` | the sidebar's background and directory names ([0012](0012-workspace-mode.md)); written `ui.rail*` between [0049](0049-inline-threads-and-the-rail.md) and [0057](0057-the-sidebar.md) |
| `ui.list.active` | `bg` of the selected entry while its list owns the keys: files, threads pane (including file groups), review file/thread headers and folded rows, and every picker result ([0079](0079-list-focus-language.md)) |
| `ui.list.inactive` | quieter `bg` of the remembered selected entry while its list does not own the keys, including under overlays, Compose, and pending key prefixes (0079) |
| `ui.list.cursor` | `fg` of the active list selection's left-edge bar, including the selected review thread's header and message rows together without replacing message author stripes; the marker ignores this role's `bg` so the row stays one band; ancestor-file context bars stay muted (0079) |
| `ui.list.hover` | subtle `bg` of hovered help and menu entries; no keyboard selection or cursor bar (0079) |
| `ui.popup`, `ui.popup.key` | picker, help, and status popup surface, and key labels (0012); the space menu drew on `ui.popup` before [0056](0056-the-leader-trimmed.md) |
| `ui.menu` | the `Space` menu and the right-click menu's surface; the built-ins share its visual ground with `ui.popup`, while a custom theme may set either independently; with no `bg` the terminal shows through ([0056](0056-the-leader-trimmed.md)) |
| `ui.picker.match` | matched characters in pickers and help filtering (0012, [0078](0078-all-keys-stays-reachable.md)) |
| `thread.open`, `thread.resolved` | gutter note cell, list rows, and file-threads rows of an open or resolved thread ([0013](0013-annotation-storage-and-ux.md)); `annotation.resolved.auto`, `annotation.detached`, and `annotation.edited` were removed by [0039](0039-gutter-colour-and-detached-rows.md) |
| `thread.waiting` | gutter note cell, list rows, and tree-pane tag of an open thread whose newest message is an agent's ([0030](0030-waiting-threads.md)); in the built-in themes `thread.open` is the hue of `thread.user` and `thread.waiting` that of `thread.agent` ([0071](0071-author-stripes.md)) |
| `thread.focus` | the threads pane's current-file context tint ([0066](0066-one-circle-language.md)), overridden by actual active or remembered list selection (0079); the rows of the thread the cursor is on before [0074](0074-the-bracket-marks-the-focused-thread.md) ([0033](0033-open-thread-lines.md)); `thread.line`, the background of annotated rows (0013), was removed by 0074 |
| `thread.bracket` | background of the gutter's note cell on the rows of the thread the cursor is on, where it draws a glyph ([0074](0074-the-bracket-marks-the-focused-thread.md)) |
| `thread.inline` | background of a thread's stub rows under its lines; `none` marks them with `▎` instead ([0049](0049-inline-threads-and-the-rail.md)) |
| `thread.user`, `thread.agent` | a message by the user or by an agent, in the expanded thread and the review list: `fg` the author's name, `bg` the stripe under the message's rows ([0071](0071-author-stripes.md)) |
| `thread.draft` | background of a draft's author row and text rows while it is written ([0071](0071-author-stripes.md)) |
| `thread.cursor` | the inline thread's `▎` bar down its cursor message and on its header or stub in the document ([0071](0071-author-stripes.md)); review selection uses `ui.list.cursor` since 0079, with ancestor-file bars in muted `ui.statusline.info` |
| `ui.header` | the neutral background of pane headers and key bars: the review list's header, unselected thread entry headers, and key bar, the files and threads pane titles, the threads pane and text key bars, the checkpoint header, an expanded thread's header, and the file-info pane's path row; selected list entries use the shared list roles (0079); the draft's author row moved to `thread.draft` in [0071](0071-author-stripes.md) ([0059](0059-headers-and-the-key-bar.md), [0067](0067-the-texts-key-bar.md)) |
| `markup.heading` | all heading levels; `markup.heading.1`…`.6` override one level |
| `markup.raw.inline`, `markup.raw.block` | inline code, code block lines |
| `markup.link` | link text |
| `markup.list` | layout chrome: bullets, table borders, quote bars, rules |
| `markup.quote` | quoted text |

An unknown key, unknown modifier, unknown colour, or unknown top-level node
is an **error with a file location** and Fathomable refuses to start, as
0008 requires for config. `--doctor` reports the theme parse result.

Since [0079](0079-list-focus-language.md), `ui.sidebar.selected` and
`ui.picker.selected` are unknown keys, not compatibility aliases
([0051](0051-retire-one-release-compatibility.md)). Remove both from a
custom theme. Inherit the built-in list styles or move the selected
background to `ui.list.active`, then set a quieter `ui.list.inactive`
background, a `ui.list.cursor` foreground, and a subtle `ui.list.hover`
background. `ui.picker.match`, `ui.sidebar`, and `ui.sidebar.dir` remain.

### Selection and built-ins

- `default-dark` and `default-light` are compiled into `fathomable-core`
  from `crates/fathomable-core/themes/`; nothing is written to disk.
- The built-ins use one `overlay` palette colour for both `ui.popup` and
  `ui.menu`, one regular-weight `key` colour for `ui.popup.key` and
  `ui.picker.match`. Dark uses slate `#1b222c` and muted blue `#8faecb`;
  light uses `#edf2f7` and `#3e6485`. The dark
  normal text, key, and subdued-info contrasts on the overlay are 10.38:1,
  6.93:1, and 4.64:1; the light equivalents are 14.63:1, 5.54:1, and
  4.80:1.
- List focus has dedicated colours, distinct from `ui.header`; active
  selection, remembered selection, and hover no longer share one surface.
  Active and inactive built-in styles set only `bg`, with no forced bold;
  the active cursor bar is the non-colour focus cue:

  | Role and channel | `default-dark` | `default-light` |
  | --- | --- | --- |
  | `ui.list.active` `bg` | `#26384a` | `#c9dff2` |
  | `ui.list.inactive` `bg` | `#202830` | `#e2e9ef` |
  | `ui.list.hover` `bg` | `#263342` | `#dce6ef` |
  | `ui.list.cursor` `fg` | `#9bc3ed` | `#1f5fbf` |

- Palette names and the list colour values above are built-in authoring
  choices, not additional schema keys. A custom theme may preserve the
  shared treatment by setting both
  semantic surfaces and both accent roles to its own common palette names,
  or deliberately separate them. Because inherited styles have already been
  resolved, overriding a palette name in a child does not recolour a parent
  style unless the child also sets that semantic style.
- A theme name resolves first to `$XDG_CONFIG_HOME/fathomable/themes/<name>.kdl`,
  then to a built-in, so a user file can shadow a built-in.
- `theme "name"` in `config.kdl` picks the default; `--theme NAME` wins for
  one run; with neither, `default-dark` is used. Detecting the terminal
  background (OSC 11) to choose light or dark automatically is parked.

### Crate boundary

`fathomable-core` owns parsing and resolution and exposes its own `Color`,
`Style`, and `Theme` types; the binary converts them to ratatui styles. No
ratatui type appears in the schema.

## Consequences

- `kdl` joins `fathomable-core`'s dependencies (approved in
  [0001](0001-dependency-policy.md)); `miette` comes with it transitively.
- `config.kdl` gets its first reader. Per 0008, nodes other than `theme` are
  errors until the setting they name is implemented.
- 0006's gutter and 0005's annotations add keys to the table above rather
  than inventing colour handling.
- `syntect` highlighting can now proceed, keyed by `code.syntect`.
