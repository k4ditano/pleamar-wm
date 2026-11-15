# Build's look: a theme of your own

Build (`pleamar-wm build`, Super+B) has a look of its own: dark, with a mint
light. A desktop, or anyone, can give it theirs —its colours, its font, rounder
or squarer corners, glass— and choose how it starts, without touching
pleamar-wm. With no theme, Build looks as it always has.

## Where it goes

Everything is in pleamar's config folder, `$PLEAMAR_CONFIG`, or
`~/.config/pleamar` (`$XDG_CONFIG_HOME/pleamar`) when that is not set:

```
~/.config/pleamar/
├── build.conf                # the model (the control centre writes it)
├── build-theme.conf          # font, corners, glass, how it starts
└── build-theme/
    ├── theme.plm             # the colours
    ├── *.ttf · *.otf · *.ttc # fonts, if the family is not installed
    └── figures/*.svg · *.png # copied next to the scene (Build's own icons are still drawn: see below)
```

`build/theme-example/` in this repository is Build's own look written as a
theme: copy it there and change it.

```sh
mkdir -p ~/.config/pleamar/build-theme
cp build/theme-example/theme.plm ~/.config/pleamar/build-theme/
cp build/theme-example/build-theme.conf ~/.config/pleamar/
```

**When it is read.** Each time Build starts (the first Super+B, or after it has
been left with Exit and has quit). A theme changed while Build is open is seen
the next time it starts.

## `theme.plm`: the colours

A pleamar library called `BuildTheme`, with one `let` for each colour Build
paints with. **All of them have to be there.** If the scene does not load with
your theme (a missing colour, a typo), Build says so in its log
(`build · …/theme.plm does not load, so Build looks as its own: …`) and uses
its own. To check one before using it, put it next to a copy of the scene
and run `pleamar --check build.plm`.

| Name | What it paints |
|---|---|
| `ink` | the words |
| `dim` | the words that say less (the model, a list's second line) |
| `soft` | what the builder did, in the thread |
| `mint` | the accent: the breathing edge, Done, the strokes, the main buttons |
| `onaccent` | words and signs on the accent and on `amber` (Done under the pointer, Allow, Keep it, send) |
| `amber` | BUILDING, and a card that asks you |
| `coral` | what went wrong, Throw away |
| `coal` | the plate of the bar, the panel and the ask box (and the glass's tint) |
| `glass` | (not used yet) |
| `veil` | the dim over the desktop while drawing or asking |
| `btn` · `btn_lit` | a button on the bar, at rest and chosen or under the pointer |
| `btn_go` | Done before the pointer is on it |
| `btn_warn` | Clear and Exit under the pointer |
| `head_btn` | the buttons in the panel's head |
| `tool_btn` | the buttons of a row, and paste and screenshot by where you write |
| `mine` | what you said, in the thread |
| `card` · `deny_btn` | a card that asks you, and its Not this |
| `row_bg` · `row_lit` | a row of What you built, at rest and under the pointer |
| `share_lit` · `del_lit` | Share and × of a row under the pointer |
| `sw_off` · `sw_on` | a row's switch |
| `field` | where you write |

Some colours in between are mixed by the scene from these (`mix(btn, btn_lit,
glow)` as the pointer comes, `mix(dim, mint, kept)`), so they follow a theme
too. The breathing edge and the strokes are `mint` at less opacity.

## `build-theme.conf`: font, corners, glass, how it starts

`key = value` or `key value`, one a line; `#` starts a remark.

| Key | Values | By default |
|---|---|---|
| `font` | a family name (`Inter`, `"IBM Plex Sans"`) | empty: the default family, as ever |
| `corners` | a factor for every corner, `0.5` to `2` | `1` |
| `glass` | `on` · `off`: the bar, the panel and the ask box of glass, tinted with `coal` | `off` |
| `start` | `draw` · `panel` | `draw` |

The family has to be installed, or be a font file in `build-theme/`; one that
is in neither is drawn in a fallback that may not be what you meant.

**`start = panel`** («panel first»): Super+B opens Build asking what to build,
in a box in the middle of the screen, as a search does. Enter builds it in a
widget's place in the middle of the monitor (move it or draw another place
later), and what you wrote goes to the builder as your first message. **Draw
it**, Esc, or the bar's pen goes to drawing, as ever. `start` can also be in
`build.conf`, and there it has the last word (it is the user's; the theme may
be a desktop's). `pleamar-wm build ask` opens it this way once, whatever
`start` says.

## What a theme does not change

- **The words.** Build writes in English and in Spanish
  (`build/lang/es.plm`, chosen by the system's language). A theme does not
  bring words of its own.
- **The icons** of the bar (hand, pen, box, ✕) and the panel are drawn in the
  scene, not taken from files: `figures/` is copied next to the scene, but
  Build's own scene does not use it yet.
- **The layout**: sizes, where the bar and the panel are, the animations.
- **What Build does**: the engine, the rules, the cards that ask.
