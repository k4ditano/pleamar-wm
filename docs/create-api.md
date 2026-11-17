# `pleamar-wm create`: Build's engine for a desktop's own interface

Build (Super+B) makes pieces of the desktop by talking with an AI. `pleamar-wm
create` gives the same engine to a desktop that wants to draw the conversation
itself — its own «create with AI» window, its own cards, its own catalog — and
to host what is made the way it hosts everything else.

The split is simple:

- **pleamar-wm** keeps the drafts, their revisions and their history, speaks to
  the engine (in its sandbox, as Build does), asks before anything that needs
  asking, and checks each revision. **It never starts what it makes.**
- **the host** (the desktop) says what kinds of thing it makes, draws the
  conversation, answers the questions, shows the preview where the piece will
  go, and runs it once it is kept.

Build's own panel is not touched by any of this: without `create`, Build is as it
was.

Every command prints JSON, one object a line, on stdout. A failure is a line
`{"type":"error","error":CODE,"message":…}` and exit 1 (2 for a command
written wrong). To know whether a pleamar-wm has it, see «Telling a
pleamar-wm that has it». Nothing it
prints holds a credential.

## Kinds

A kind is what the host makes: a desktop widget, a bar item. It is a file of
the host's, `$PLEAMAR_CONFIG/create-kinds/KIND.json` (`~/.config/pleamar/…` by
default):

```json
{
  "name": "Desktop widget",
  "description": "a card on the desktop, under the windows",
  "main": "scene.plm",
  "first_line": "// my desktop widget",
  "template": "widget-template.plm",
  "guide": "widget-guide.md",
  "sizes": { "small": [164, 164], "medium": [344, 164], "large": [344, 344] },
  "default_size": "small",
  "requires": ["widget.json"],
  "allow_run": ["curl"]
}
```

| Key | What it says |
| --- | --- |
| `name`, `description` | What it is, for the engine and for the host's own lists |
| `main` | The scene the piece is (`scene.plm` if not said) |
| `first_line` | The line its main scene must start with (the host's mark), checked |
| `template` | Where a new draft starts: `{{W}}`, `{{H}}` and `{{NAME}}` in it are replaced |
| `guide` | The host's guide for it, handed to the engine with every conversation: its look, its components, its rules |
| `sizes`, `default_size` | The sizes it comes in, `[width, height]`; `begin --size` takes a name or `W,H` |
| `requires` | Files it must have (a manifest, for instance), checked |
| `allow_run` | If said, the only programs its scene may run (`permissions { run: … }`), checked |
| `copy` | Folders of the host's copied in beside its files, afresh at `begin`, each turn, `check` and `keep`: `{"lib": "~/.config/my-desktop/widgets/lib"}` puts it in `DIR/lib`, so the scene's `include "lib/…"` resolve for the check and the preview |
| `ignore` | Problem codes of the check the host takes care of itself (`["shadow_cut"]` when it draws the shadow outside) |

Relative paths are beside the kind's file; `~/` is the home.

## Commands

| Command | Answer |
| --- | --- |
| `create kinds` | `{"type":"kinds","kinds":[{id, name, description, sizes, default_size}]}` |
| `create begin KIND [--size NAME\|W,H] [--name NAME]` | A new draft from the kind's template: `{"type":"begun", id, kind, name, size, state:"draft", rev, dir, main}`. `dir` is where its files are |
| `create begin --from ID` | A new conversation over one that exists (to change it), its history kept: `state:"editing"` when it had been kept |
| `create prompt ID TEXT` | The conversation's turn: stays, printing its events (below), until `done`, `cancelled` or `error` |
| `create permit ID REQ once\|always\|deny` | The answer to a question the running `prompt` asked: `{"type":"permitted", req, decision}` |
| `create cancel ID` | Stops the running turn: `{"type":"cancelling"}`; its `prompt` ends with `cancelled` |
| `create check ID` | `{"type":"check", rev, ok, problems:[{code, file, message}]}` |
| `create keep ID` | The draft becomes the creation: `{"type":"kept", id, dir, main, kept_rev, permissions:{run, services}, manifest, warnings}`. Refused (`not_ready`) while the check finds what keeps it from running |
| `create undo ID` | Back one revision of this conversation: `{"type":"revision", rev, …}` (never below what was kept) |
| `create discard ID` | A draft: gone. One being edited: back to what was kept |
| `create remove ID` | A creation and all its revisions, gone |
| `create list` | `{"type":"list","creations":[{id, kind, name, size, state, rev, kept_rev, dir, main, updated, manifest, made_by}]}` newest first. `manifest` is the creation's `widget.json` when it has one (`null` if not), `made_by` its `made_by` (`ai` if it does not say) |
| `create get ID` | All of it: the above plus `files`, `revs` (`{rev, why, at}`), `history` (`{role, text, at}`), `approved`, `busy` |

`state` is `draft` (never kept), `kept`, or `editing` (kept, and a conversation
over it going on). One conversation at a time per creation: another command
that would change it answers `busy`.

### The events of `prompt`

| Event | When |
| --- | --- |
| `status {code, text}` | What it is doing now, one line for the host's compact view. `code`: `starting`, `thinking`, `reading`, `writing`, `checking`, `looking`, `reading_docs`, `running`, `waiting_permission`, `mending` |
| `message {role:"assistant", text}` | What the engine said, for the history (also kept: `get`) |
| `permission {req, what, …}` | A question; the turn waits for `permit` (a quarter of an hour, then it is «no»). `what` is `scene` (its permissions: `asks:[{key:"run"\|"services", value, words}]`, `summary`), `run` (`command`, `words`) or `read` (`command`: the path). Each has `why` (the engine's reason, in the user's language) and `domains`: the web addresses involved, to say plainly what it is for («api.open-meteo.com») |
| `revision {rev, dir, files, why, domains}` | The files changed: the host shows this revision (a preview where it goes) |
| `named {name}` | The engine gave it a name |
| `check {rev, ok, problems}` | The check after a turn that changed it; when it fails, the engine is told what was found and mends it, up to two rounds (`status mending`) |
| `done {rev, changed, summary, why}` | The turn ended: `summary` is its last words, for the host to show as the result |
| `cancelled {rev}` | `cancel` stopped it |
| `error {error, message}` | It could not go on: `auth_required` (with `reason`: nobody signed in, the session expired…: `pleamar-wm build login`, docs/build-access.md), `engine_failed`, `timeout`, `busy`. The draft is left as it was |

`once` allows that one thing; `always` allows it for this creation from then on
(a program with those arguments, a permission of its scene). A scene's
permissions allowed once are allowed for the creation: its scene keeps asking
them. What is refused outright (deleting files, the administrator, a disk, a
password) is never asked: the engine is told no and finds another way.

### The check

- its main scene exists and starts with the kind's `first_line`;
- the kind's `requires` are there, and every `.json` file is valid JSON;
- `pleamar --check` passes;
- its permissions are allowed (Build's rules, and the kind's `allow_run`);
- where pleamar has it, `pleamar --layout`: text over text, text cut or outside
  its surface, too little contrast, a shadow cut by its surface.
  Its warnings come too, with `"level":"warning"`: texts nearly but not quite on
  one edge (`misaligned`), a gap in a column of texts off the others
  (`uneven_spacing`). The engine is told them to mend, but they never keep a
  creation from being kept, and `ok` counts only what is not a warning.

### What the engine may do

As in Build: read and write the creation's files (`.plm`, `.luau`, `.svg`,
`.json`), check it, look at it (a picture taken with no screen, where pleamar
can film), name it, read pleamar's documents, and —asked first when Build's
rules say so— run a program or read a file of the user's. Keys and using it with
the agent's cursor are not available here: the host shows it and uses it.

## Where things are

```
$XDG_DATA_HOME/pleamar-wm/create/ID/state.json   what it is, its revisions, history and what was allowed
$XDG_DATA_HOME/pleamar-wm/create/ID/files/       its files as they are now (`dir`)
$XDG_DATA_HOME/pleamar-wm/create/ID/revs/N/      each revision
```

## Telling a pleamar-wm that has it

Ask `pleamar-wm build --version` (safe on any pleamar-wm since 0.6.25, which
answers it without doing anything): one that has `create` says so,
`{"type":"version","version":…,"commands":[…],"create":["kinds","begin",…]}`.
Without that `create` key, do not call `pleamar-wm create`: an older one does
not know the word.

Before 0.6.25 any word after `build` turned build mode on —`build --version`
too—, so a host that may meet an old one looks at the program first, without
running it (docs/build-access.md). Only a pleamar-wm that has `create` carries
its error words:

```sh
grep -qa nothing_to_undo "$(command -v pleamar-wm)" && echo "has create"
```

## Trying it without AI

`PLEAMAR_BUILD_ENGINE`, a command, takes the engine's place, as for `build
status`: `tests/create_api.rs` has a fake one that writes a scene, asks, names
it and is done.

## A minimal host

```sh
id=$(pleamar-wm create begin my-widget | jq -r .id)
pleamar-wm create prompt "$id" "the weather in Madrid, small" | while read -r ev; do
  case $(echo "$ev" | jq -r .type) in
    status)     echo "… $(echo "$ev" | jq -r .text)" ;;
    permission) pleamar-wm create permit "$id" "$(echo "$ev" | jq -r .req)" once ;;  # ask the user here
    done)       echo "$(echo "$ev" | jq -r .summary)" ;;
  esac
done
pleamar-wm create keep "$id"     # then run it as the host runs its own pieces
```
