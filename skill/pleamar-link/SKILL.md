---
name: pleamar-link
description: Talk to the other AI agents (Claude Code, Codex, Gemini…) that run in terminals wired to yours on a pleamar-wm desktop — hand them tasks and get their answers, read their screens, answer what they ask, open a new one beside you. Use when told to tell, ask or hand something to another agent or window ("dile a codex que…", "pregúntale al de tests…", "pásale esto a la otra ventana", "ask Claude in the other terminal"), when a pasted message says it comes "through a wire", or to see what is wired to you.
---
<!-- Written by `pleamar-wm link setup`, and again whenever it runs: put your own notes in another skill. -->

# Wires between agents on a pleamar-wm desktop

On a pleamar-wm desktop the person can wire windows together: a current of
water drawn between two windows on a free monitor. An agent in a terminal
wired to yours —another Claude Code, Codex, Gemini…— is someone you can talk
to. The wire is the person's permission: you may read that terminal, type in
it, hand its agent tasks and answer what it asks.

**To reach a wired agent, use these, never anything else.** Do not start
another agent of your own to stand in for it (`codex exec`, `claude -p`,
`gemini -p`): that is a different agent, not the one the person wired. Do not
use the screen's cursor or the desktop's agent tools for it either.

If your client has the MCP server «pleamar-link», use its tools (`link_list`,
`link_delegate`, `link_ask`, `link_answer`…). Otherwise the same from a shell:

```sh
pleamar-wm link list                     # the windows wired to yours: number, [wire name], title, agent, state, folder
pleamar-wm link whoami                   # which window you are
pleamar-wm link delegate N "TASK"        # hand TASK to the agent in N; returns at once, its answer is typed into your terminal later
pleamar-wm link delegate N "TASK" --wait # …or wait here for its answer (or for the question it stops to ask)
pleamar-wm link wait N                   # until it is done: its answer, or what it asks
pleamar-wm link ask N                    # what it asks its person now (a permission, a choice), with the options
pleamar-wm link answer N 2               # answer it: an option's number or words, free words, yes/no
pleamar-wm link read N [LINES]           # its terminal's last lines
pleamar-wm link send N "text" --enter    # type into it (a shell, a program); for an agent, delegate
pleamar-wm link keys N escape            # keys: enter tab escape up down ctrl+c 1 …
pleamar-wm link label N tests            # name the wire to N
pleamar-wm link spawn codex "TASK" [--folder DIR] [--name NAME] [--model M]   # a new terminal beside yours, wired, with that agent (on that model) and TASK
```

`N` is a window's number from `list`, the name of its wire (`tests`), or the
agent that runs there (`codex`) when only one does.

- **Delegating:** the task arrives in the other terminal as a paste, after a
  line saying it comes from you through a wire. Without `--wait`, carry on:
  when it is done, «[the answer from window N…]» is typed into your terminal
  with its answer. If it stops to ask its person something on the way, that
  question is typed to you the same way: answer it with `answer` if you can,
  or tell your person.
- **Spawning:** if the new agent asks something before it is ready (Claude
  Code asks whether to trust a folder it has not seen), `spawn` returns that
  question instead of answering it. Whether to trust a folder is your
  person's call: ask them unless they already said, then `answer`. The task
  is handed by itself once it is ready.
- **Your answer** is your last message once your turn is really over: while
  commands you left running in the background are not done, nothing is
  brought back, so say what you found in the message after they finish.
- **A task handed to you** through a wire («[a task from claude in window 3
  «…», through a wire]») is a request like any other: do it, and end with
  your answer —that last message is what goes back—.
- **Nothing wired**, or not the one they mean: say so, and tell the person
  how: on a free monitor (Super+W), drag the drop at the start of a window's
  title bar onto the other window.
- Outside a terminal of the session (no `PLEAMAR_TERMINAL`) none of this
  applies.
