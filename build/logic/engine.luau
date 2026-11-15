-- ── the engine: the AI that builds, and the conversation with it ──
--  The Pi SDK in a sandbox (agent/), spoken to in JSON lines. It PROPOSES;
--  this decides. What it does to the creation is done at once; what it asks
--  of the system goes through creations.judge (fixed rules) and, when they
--  say so, becomes a card you allow or not. What happened is what this says
--  happened, never what the model says.
return function(shared)
    local home_dir = shared.home_dir
    local base = sys.ask("env", "XDG_STATE_HOME")
    if base == nil or base == "" then base = home_dir .. "/.local/state" end
    local STATE = base .. "/pleamar-wm/build"
    shared.STATE = STATE
    --  The model: the control centre's choice (~/.config/pleamar/build.conf),
    --  the strongest there is if none, and whatever this account has after it.
    local DEFAULT_MODEL = "openai-codex/gpt-6-astra"
    local chosen_model = DEFAULT_MODEL
    local function read_conf(done)
        run("sh", { "-c", 'cat "${PLEAMAR_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/pleamar}/build.conf" 2>/dev/null; true' }, function(out)
            for line in tostring(out or ""):gmatch("[^\n]+") do
                local m = line:match("^%s*model%s+(%S+)")
                if m then chosen_model = m end
            end
            if done then done() end
        end)
    end
    read_conf()

    local ROWS = 30
    local rows = {}
    local history = {}
    local worker, ready = nil, false
    local queue = {}
    local live = nil
    local shown_row, shown, revealing = nil, 0, false
    local asks = {}          -- proposal id -> { row, tool, args } (or a card of our own: then_)
    local idle_timer = nil
    local E = {}

    local function letters(s)
        local ok, n = pcall(utf8.len, s)
        return ok and n or #s
    end
    local painting = false
    local function paint()
        if painting then return end
        painting = true
        after(16, function()
            painting = false
            local first = math.max(1, #rows - ROWS + 1)
            local out = {}
            for k = first, #rows do
                local r = rows[k]
                out[#out + 1] = { kind = r.kind, text = r.text, detail = r.detail or "", state = r.state or 0 }
            end
            model.rows = out
            if shown_row and shown_row >= first then
                fact["c.live"] = shown_row - first
                fact["c.letters"] = shown
            else
                fact["c.live"] = -1
            end
        end)
    end
    local TAIL = 12
    local function reveal()
        if revealing then return end
        revealing = true
        local function tick()
            if not shown_row or not rows[shown_row] then revealing = false return end
            local total = letters(rows[shown_row].text)
            local behind = total - shown
            if behind <= -TAIL then
                if shown_row ~= live then shown_row = nil; revealing = false; paint() return end
            else
                shown = shown + math.clamp(math.floor(behind / 12), 2, 10)
                paint()
            end
            after(33, tick)
        end
        after(33, tick)
    end
    local function add(kind, words, detail, state)
        rows[#rows + 1] = { kind = kind, text = words or "", detail = detail or "", state = state or 0 }
        paint()
        return #rows
    end
    local function status(words) text["c.status"] = words or "" end
    local function finish_row(row, ok, denied)
        if row and rows[row] then rows[row].state = denied and 3 or (ok and 1 or 2); paint() end
    end
    local function send(m)
        local line = json.encode(m) .. "\n"
        if worker and ready then
            if not write(worker, line) then log("build · the engine did not hear") end
        else
            queue[#queue + 1] = line
        end
    end
    local function answer(id, ok, words, extra)
        local m = { type = "result", id = id, ok = ok, text = words or "" }
        if extra then for k, v in pairs(extra) do m[k] = v end end
        send(m)
    end

    -- ── its tools, done ──
    local function perform(id, tool, args, row)
        fact["c.state"] = "acting"
        status(shared.creations.said(tool, args))
        shared.creations.perform(tool, args, function(ok, words, extra)
            finish_row(row, ok)
            answer(id, ok, words, extra)
        end)
    end

    -- ── the worker ──
    local function stop_idle() if idle_timer then cancel(idle_timer); idle_timer = nil end end
    local function rest_later()
        stop_idle()
        idle_timer = after(5 * 60 * 1000, function()
            idle_timer = nil
            if worker and (fact["c.state"] == "idle" or fact["c.state"] == "offline") then write(worker, json.encode({ type = "shutdown" }) .. "\n") end
        end)
    end
    local function settle(state)
        fact["c.state"] = state or "idle"
        status("")
        live = nil
        paint()
        shared.creations.save_talk(rows, history)
        rest_later()
    end
    local function on_event(m)
        local kind = m.type
        if kind == "ready" then
            ready = true
            text["c.model"] = tostring(m.model or ""):gsub("^[^/]+/", "")
            if m.usable then
                if fact["c.state"] == "offline" or fact["c.state"] == "starting" then fact["c.state"] = "idle"; status("") end
            else
                fact["c.state"] = "offline"
                status(m.reason == "signed_out" and tr("Not signed in: pleamar-wm's control centre › Build, to sign in") or tr("That model cannot be used: choose another in the control centre › Build"))
            end
            for _, line in ipairs(queue) do write(worker, line) end
            queue = {}
        elseif kind == "delta" then
            if live == nil then
                live = add(2, "")
                shown_row, shown = live, 0
                fact["c.state"] = "thinking"
                status(tr("Thinking…"))
            end
            rows[live].text = rows[live].text .. (m.text or "")
            reveal()
        elseif kind == "propose" then
            live = nil
            local tool, args = tostring(m.tool), (type(m.args) == "table" and m.args or {})
            local verdict = shared.creations.propose(tool, args)
            if verdict and verdict.refuse then
                if verdict.note and verdict.note ~= "" then add(5, verdict.note) end
                answer(m.id, false, verdict.refuse)
            elseif verdict and verdict.ask then
                local row = add(4, verdict.ask, verdict.detail, 0)
                asks[m.id] = { row = row, tool = tool, args = args }
                fact["c.state"] = "awaiting"
                status(tr("Waiting for you"))
            else
                local row = add(3, shared.creations.said(tool, args), tool == "creation_write" and tostring(args.why or "") or "", 0)
                perform(m.id, tool, args, row)
            end
        --  Its turn over (done, stopped or failed): the agent's cursor it
        --  tried things with goes too.
        elseif kind == "done" then
            shared.creations.release_cursor()
            if live then history[#history + 1] = { role = "assistant", text = rows[live].text } end
            settle("idle")
        elseif kind == "cancelled" then
            shared.creations.release_cursor()
            add(5, tr("Stopped."))
            settle("idle")
        elseif kind == "failed" then
            shared.creations.release_cursor()
            if m.reason == "no_model" or m.reason == "not_started" then
                add(5, tr("Not signed in: pleamar-wm's control centre › Build, to sign in"))
            else
                add(5, tr("Something went wrong: ") .. tostring(m.reason or "?"))
            end
            settle("idle")
        end
    end
    --  Its packages, fetched the first time (and when its lock changes).
    local function prepared(done)
        run("sh", { "-c", 'cd agent && { [ -d node_modules/@earendil-works/pi-coding-agent ] && [ ! package-lock.json -nt node_modules/.package-lock.json ]; } || { command -v npm >/dev/null || { echo "npm is missing: install Node.js (and npm) to build with AI"; exit 3; }; npm ci --ignore-scripts --no-audit --no-fund >/dev/null 2>&1 || { echo "the engine could not be installed (npm ci)"; exit 4; }; }' }, function(out, code)
            done(code == 0, tostring(out or ""):gsub("%s+$", ""))
        end, { errors = true })
    end
    local function start_worker()
        if worker then return end
        ready = false
        fact["c.state"] = "starting"
        status(tr("Waking the builder up…"))
        prepared(function(ok, why)
            if not ok then
                fact["c.state"] = "offline"
                status(why)
                queue = {}
                return
            end
            read_conf(function()
                local said_why = nil
                local good, id = pcall(spawn, "sh", { "agent/sandbox" }, function(line)
                    if line:sub(1, 1) ~= "{" then
                        log("build · engine · " .. line)
                        if line:sub(1, 7) == "build: " then said_why = line:sub(8) end
                        return
                    end
                    local okj, m = pcall(json.decode, line)
                    if okj and type(m) == "table" then on_event(m) end
                end, function(_, code)
                    log("build · the engine ended", code)
                    worker, ready = nil, false
                    if said_why then
                        fact["c.state"] = "offline"
                        status(tr("The builder cannot start: ") .. said_why)
                        queue = {}
                        return
                    end
                    if fact["c.state"] ~= "idle" and fact["c.state"] ~= "offline" then
                        add(5, tr("The builder stopped."))
                        settle("idle")
                    end
                end, { stdin = "open", errors = true, env = { BUILD_AGENT_STATE = STATE } })
                if not good then
                    fact["c.state"] = "offline"
                    status(tr("The builder cannot start: ") .. tostring(id))
                    return
                end
                worker = id
                local recent = {}
                for k = math.max(1, #history - 40), #history do recent[#recent + 1] = history[k] end
                write(worker, json.encode({ type = "start", model = chosen_model, fallback = "openai-codex/gpt-6-astra", locale = fact.locale,
                                            history = recent, build = shared.creations.context() }) .. "\n")
            end)
        end)
    end

    -- ── what you say ──
    local function send_input()
        local words = tostring(text["c.input"] or "")
        if words:match("^%s*$") then return end
        local state = fact["c.state"]
        if state == "thinking" or state == "acting" or state == "awaiting" or state == "starting" then return end
        if not shared.creations.active() then return end
        text["c.input"] = ""
        local images, pictures, about = shared.creations.take_refs()
        add(1, pictures ~= "" and (words .. "\n📎 " .. pictures) or words)
        history[#history + 1] = { role = "user", text = words }
        stop_idle()
        start_worker()
        fact["c.state"] = "thinking"
        status(tr("Thinking…"))
        --  What each picture is goes with the words (not on the screen).
        send({ type = "prompt", text = about and (words .. "\n\n" .. about) or words, now = os.date("%Y-%m-%d %H:%M, %A"), images = images })
    end
    on("c_send", send_input)
    --  What you asked for first, before the panel (draw.luau's «panel first»).
    shared.say = function(words) text["c.input"] = words; send_input() end
    local function decide(i, yes)
        local first = math.max(1, #rows - ROWS + 1)
        local row = first + (i or 0)
        for id, a in pairs(asks) do
            if a.row == row then
                asks[id] = nil
                if a.then_ then
                    rows[row].state = yes and 3 or 2
                    paint()
                    if next(asks) == nil then fact["c.state"] = "idle"; status("") end
                    a.then_(yes)
                    return
                end
                if yes then
                    rows[row].state = 3
                    local tool_row = add(3, shared.creations.said(a.tool, a.args), tostring(a.args.why or ""), 0)
                    perform(id, a.tool, a.args, tool_row)
                    if next(asks) ~= nil then fact["c.state"] = "awaiting"; status(tr("Waiting for you")) end
                else
                    rows[row].state = 2
                    paint()
                    answer(id, false, "", { denied = true })
                    fact["c.state"] = next(asks) ~= nil and "awaiting" or "thinking"
                end
                return
            end
        end
    end
    on("c_allow", function(i) decide(i, true) end)
    on("c_deny", function(i) decide(i, false) end)
    on("c_stop", function()
        for id, a in pairs(asks) do
            asks[id] = nil
            if rows[a.row] then rows[a.row].state = 2 end
        end
        send({ type = "cancel" })
        status(tr("Stopping…"))
    end)

    -- ── what creations borrows ──
    --  A creation's own conversation: begun with what it had, and handed
    --  back when it ends. The engine starts again for each.
    local function restart()
        if worker then write(worker, json.encode({ type = "shutdown" }) .. "\n") end
        asks, live = {}, nil
        shown_row, shown = nil, 0
        fact["c.state"] = "idle"
        status("")
    end
    function E.begin(old_history, old_rows)
        restart()
        rows, history = old_rows or {}, old_history or {}
        if #rows == 0 then add(2, tr("Tell me what it should be: its colours, its look, what it shows and does. I'll make it right there, as you watch.")) end
        paint()
    end
    function E.finish()
        local r, h = rows, history
        restart()
        rows, history = {}, {}
        paint()
        return r, h
    end
    function E.note(words) add(3, words, "", 1) end
    function E.error(words) add(5, words) end
    function E.card(body, detail, then_)
        local row = add(4, body, detail, 0)
        asks["local-" .. row] = { row = row, tool = "__local", args = {}, then_ = then_ }
        fact["c.state"] = "awaiting"
        status(tr("Waiting for you"))
    end
    --  A first message of the builder's own, with the drawing: so it starts
    --  from what you drew even before you say anything.
    function E.prompt_with(images, words)
        stop_idle()
        start_worker()
        fact["c.state"] = "thinking"
        status(tr("Thinking…"))
        history[#history + 1] = { role = "user", text = words }
        send({ type = "prompt", text = words, now = os.date("%Y-%m-%d %H:%M, %A"), images = images })
    end
    function E.stop_worker() restart() end
    shared.engine = E
    return E
end
