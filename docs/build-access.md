# Build's AI: signing in and choosing the model, from any shell

Build (`pleamar-wm build`, Super+B) builds with an AI that you sign in to:
ChatGPT (`openai-codex`), Claude (`anthropic`) or GitHub Copilot
(`github-copilot`). The control centre's Build page does it by hand
(`pleamar-wm control build` opens it there). These commands do the same for a
shell's own settings, a script or a terminal:

```
pleamar-wm build status                  whether Build can build now, and if not, why
pleamar-wm build models                  the models the signed-in accounts can build with
pleamar-wm build model PROVIDER/ID       the model Build builds with from its next start
pleamar-wm build login PROVIDER [--open] sign in (--open: the page in the browser)
pleamar-wm build logout PROVIDER         sign out
```

They start Build's own engine for the one question and stop it after: Build
need not be open. The first time, the engine's packages are fetched (Node.js and
npm, and bubblewrap for its sandbox, have to be installed), which takes a minute.

## Which pleamar-wm has these

From 0.6.25. `pleamar-wm build --version` answers what its `build` can do:

```json
{"type":"version","version":"0.6.25","commands":["on","off","ask","status","models","model","login","logout"]}
```

and a word `build` does not know is a usage error (exit 2, a JSON `usage` line),
never build mode. **Before 0.6.25 any word after `build` toggled build mode**
—`pleamar-wm build status` turned it on, and so does `build --version`—, so do
not ask an unknown pleamar-wm with them: look at the program instead, without
running it. Only a pleamar-wm that has these carries their error words:

```sh
grep -qa engine_missing "$(command -v pleamar-wm)" && echo "has build status"
```

## What they print

One JSON object a line on stdout, and exit 0. Field names are stable; new
fields may be added.

**`status`**

```json
{"type":"status","model":"openai-codex/gpt-6-sol","chosen":true,"usable":false,"reason":"expired",
 "accounts":[{"provider":"openai-codex","type":"oauth","state":"expired","reason":"OAuth refresh failed for openai-codex: … (401)"},
             {"provider":"anthropic","type":"oauth","state":"ok","reason":null}]}
```

- `model`: the one in `build.conf`, or Build's own default when it names none (`chosen: false`).
- `usable`: Build can build with that model now.
- `reason`, when it cannot:

| `reason` | What it means | What to do |
|---|---|---|
| `signed_out` | the model's account has no sign-in | `build login PROVIDER` |
| `expired` | it has one, but the provider no longer takes it (an expired or revoked session: a 400/401/403 when renewing it) | `build login PROVIDER` again |
| `unreachable` | the provider did not answer when renewing it (the network) | try again; sign in again if it goes on |
| `model_unavailable` | signed in, but that account cannot build with that model | `build models`, then `build model …` |
| `error` | something else went wrong with that account | its `reason` says what |

- `accounts`: each account with a sign-in, tried as a request would try it (a
  token about to expire is renewed then): `state` is `ok`, `expired`,
  `unreachable` or `error`, and `reason` the provider's words, cleaned (one
  line, and nothing that looks like a token).

**`models`**

```json
{"type":"models","signed":["anthropic"],"models":[{"id":"anthropic/claude-x","provider":"anthropic","chosen":false}]}
```

Only models that can see pictures: building looks at what it made.

**`model PROVIDER/ID`**

```json
{"type":"model","ok":true,"model":"anthropic/claude-x","path":"/home/me/.config/pleamar/build.conf"}
```

It changes only the `model` line of `build.conf`
(`${PLEAMAR_CONFIG:-~/.config/pleamar}/build.conf`); its other lines stay. It
does not ask the engine: a model of an account whose session has to be renewed
can still be chosen. Build takes it the next time it starts.

**`login PROVIDER`**: as the engine says them, a line with the page to finish
signing in (and the code to type there, when the provider gives one), then
the end:

```json
{"type":"login_url","provider":"github-copilot","url":"https://github.com/login/device","code":"ABCD-1234"}
{"type":"login_done","provider":"github-copilot","ok":true}
```

Show the code and open the URL (`--open` opens it with `xdg-open`). It waits
five minutes for the sign-in to be finished.

**`logout PROVIDER`**

```json
{"type":"logout","provider":"openai-codex","ok":true,"signed":["anthropic"]}
```

## When it fails

A line `{"type":"error","error":CODE,"message":"…"}` and exit 1 (2 for
`usage`):

| `error` | |
|---|---|
| `usage` | the command was written wrong |
| `engine_missing` | the engine could not be prepared: Node.js or npm missing, `npm ci` failed |
| `engine_failed` | the engine stopped without answering (bubblewrap or Node missing: its message says which) |
| `timeout` | it did not answer in time (90 s; 5 min for a sign-in) |
| `login_failed` · `cancelled` · `unknown_provider` | the sign-in did not finish |
| `config_unwritable` | `build.conf` could not be written |

No credential is ever printed or logged: the engine keeps them in
`${XDG_STATE_HOME:-~/.local/state}/pleamar-wm/build/pi/` (0700), and what it
says of a failure comes cleaned.

## In Build itself

When building fails because of the sign-in, Build's panel says so plainly —«The
ChatGPT session has expired: sign in again», «Not signed in: sign in to build
with AI»— with a **Sign in** button beside it that opens the control centre at
its Build page.

## A shell's settings, the least there is

```luau
-- In a scene allowed to run pleamar-wm (permissions { run: "pleamar-wm" }).
run("pleamar-wm", { "build", "status" }, function(out, code)
    local s = json.decode(out)
    if s.type == "error" then text.ai = "Build's AI: " .. s.message return end
    text.ai = s.usable and ("Building with " .. s.model)
        or ({ signed_out = "Not signed in", expired = "Sign in again",
              unreachable = "No network", model_unavailable = "Choose another model" })[s.reason] or s.reason
end)
-- Signing in: show the code, open the page; the last line says how it went.
spawn("pleamar-wm", { "build", "login", "openai-codex", "--open" }, function(line)
    local m = json.decode(line)
    if m.type == "login_url" and m.code then text.ai = "Type " .. m.code .. " in the page" end
    if m.type == "login_done" then text.ai = "Signed in" end
    if m.type == "error" then text.ai = m.message end
end, function() end)
```

`PLEAMAR_BUILD_ENGINE`, a command, takes the engine's place: tests use a fake
one (`tests/build_access.rs`) that answers from canned lines.
