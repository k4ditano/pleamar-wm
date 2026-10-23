-- pleamar-wm's control centre: what each monitor is, from the session
-- itself (`pleamar-wm --say wm "get …"`), and what is picked, applied there
-- at once and written in session.conf (`pleamar-wm config set layout …`).

local NAMES = { "leader", "columns", "rows", "grid", "leader-right", "scrolling", "marea", "fractal", "free" }
local SHOWN = { "Leader", "Columns", "Rows", "Grid", "Leader right", "Scrolling", "Marea", "Fractal", "Free" }

local function wm(args, done)
    run("pleamar-wm", args, function(out, code)
        if done then done((out or ""):gsub("%s+$", ""), code) end
    end)
end

local function shown(k, l)
    fact["sel." .. k] = l
    text["now." .. k] = l >= 0 and ("now " .. SHOWN[l + 1]) or ""
end

-- Each monitor's name, and its layout now (free counts first: a canvas).
local function look()
    for k = 0, 3 do
        wm({ "--say", "wm", "get monname." .. k }, function(name, code)
            if k == 0 then fact.reachable = code == 0 end
            fact["has." .. k] = code == 0 and name ~= ""
            text["mon." .. k] = name
            if code ~= 0 or name == "" then return end
            wm({ "--say", "wm", "get free." .. k }, function(free)
                if (tonumber(free) or 0) > 0.5 then return shown(k, 8) end
                wm({ "--say", "wm", "get lay." .. k }, function(lay)
                    shown(k, math.floor(tonumber(lay) or 0))
                end)
            end)
        end)
    end
end

-- ── Dock: how the shore of free monitors behaves (`dock-style …`) ──
-- Read from the session; a change applies there at once (`fact dock.hide 1`)
-- and is written in session.conf (`pleamar-wm config set dock hide on`).
local DOCK = { "hide", "magnify", "dots", "into", "size", "side" }
local FACT = { "dock.hide", "dock.mag", "dock.dots", "dock.into", "dock.size", "dock.side" }
local MINE = { "d.hide", "d.mag", "d.dots", "d.into", "d.size", "d.side" }
local SIDES = { [0] = "bottom", [1] = "left", [2] = "right" }
local SAID = {
    "Hiding the dock", "Magnification", "Drops under open programs", "Windows put away into their icon", "The dock's size", "The dock's place",
}

local function dock_look()
    for i = 1, #FACT do
        wm({ "--say", "wm", "get " .. FACT[i] }, function(v, code)
            local n = tonumber(v)
            if code == 0 and n then fact[MINE[i]] = n end
        end)
    end
end

on("dk", function(v)
    local k, x = v // 100, v % 100
    if k >= 6 then return agent_set(k, x) end
    local i = k + 1
    if not DOCK[i] then return end
    local value
    if k <= 3 then
        value = x == 1 and "on" or "off"
    elseif k == 4 then
        value = tostring(x)
    else
        value = SIDES[x]
    end
    if not value then return end
    fact[MINE[i]] = x
    -- At once, there…
    wm({ "--say", "wm", "fact " .. FACT[i] .. " " .. x })
    -- …and for the next session.
    wm({ "config", "set", "dock", DOCK[i], value }, function(out, code)
        local what = k <= 3 and (SAID[i] .. (x == 1 and " on" or " off")) or (SAID[i] .. ": " .. value)
        text.toast = code == 0 and (what .. ", from now on") or ("Not saved: " .. out)
        fact.toasted = fact.toasted + 1
    end)
end)

-- ── Agent: what an AI agent may do (`agent`, `a11y`, `agent pace` in session.conf) ──
-- Read from what the session was started with (`pleamar-wm config`); a change
-- is written there and shows in the next session.
local PACES = { "default", "fast", "instant" }
local function agent_look()
    wm({ "config" }, function(out, code)
        if code ~= 0 then return end
        local function field(name) return out:match("\n%s*" .. name .. ": (%w+)") end
        fact["a.agent"] = field("agent") == "true" and 1 or 0
        fact["a.a11y"] = field("a11y") == "true" and 1 or 0
        local pace = field("pace")
        fact["a.pace"] = pace == "Fast" and 1 or (pace == "Instant" and 2 or 0)
    end)
end

local function agent_saved(what, out, code)
    text.toast = code == 0 and (what .. ", from the next session") or ("Not saved: " .. out)
    fact.toasted = fact.toasted + 1
end

-- Settings 6 (its hands) and 7 (accessibility) are the dock's toggles' codes;
-- 8 is the pace.
local function agent_set(k, x)
    if k == 6 then
        fact["a.agent"] = x
        wm({ "config", "set", "agent", x == 1 and "on" or "off" }, function(out, code)
            agent_saved("The agent's own hands " .. (x == 1 and "on" or "off"), out, code)
        end)
    elseif k == 7 then
        fact["a.a11y"] = x
        wm({ "config", "set", "a11y", x == 1 and "on" or "off" }, function(out, code)
            agent_saved("The accessibility bus " .. (x == 1 and "on" or "off"), out, code)
        end)
    elseif k == 8 and PACES[x + 1] then
        fact["a.pace"] = x
        wm({ "config", "set", "agent-pace", PACES[x + 1] }, function(out, code)
            agent_saved("The agent's pace: " .. PACES[x + 1], out, code)
        end)
    end
end

local open_build = nil   -- the Build page, below: wakes its engine up
on("go", function(page)
    fact.page = page
    if page == 3 and open_build then open_build() end
    if page == 4 then dock_look() end
    if page == 5 then agent_look() end
end)

on("pick", function(v)
    local k, l = v // 10, v % 10
    local name = text["mon." .. k]
    if name == "" then return end
    shown(k, l)
    -- At once, there…
    if l == 8 then
        wm({ "--say", "wm", "fact free." .. k .. " 1" })
    else
        wm({ "--say", "wm", "fact lay." .. k .. " " .. l, "fact free." .. k .. " 0" })
    end
    -- …and for the next session.
    wm({ "config", "set", "layout", name, NAMES[l + 1] }, function(out, code)
        text.toast = code == 0 and (name .. " starts in " .. SHOWN[l + 1] .. " from now on") or ("Not saved: " .. out)
        fact.toasted = fact.toasted + 1
    end)
end)

wm({ "--version" }, function(out) text.version = out ~= "" and out or "pleamar-wm" end)
look()
dock_look()
agent_look()
-- A layout picked from the strip, or with Super+W, shows here too.
every(2000, look)

-- ── Build: who the AI is, and which model builds ──
-- The same engine as Build's (pleamar-wm build), asked who is signed in and
-- what can be used; signing in opens the provider's page in the browser.
-- The model chosen is written in ~/.config/pleamar/build.conf (`model …`).
local PROVIDERS = { "openai-codex", "anthropic", "github-copilot" }
local NAMES_OF = { ["openai-codex"] = "ChatGPT", anthropic = "Claude", ["github-copilot"] = "GitHub Copilot" }
local data = sys.ask("env", "XDG_DATA_HOME")
if data == nil or data == "" then data = (sys.ask("env", "HOME") or "") .. "/.local/share" end
local BUILD = data .. "/pleamar-wm/build"
local engine, engine_ready, waiting = nil, false, {}
local models = {}
local chosen = ""

local function conf(done)
    run("sh", { "-c", 'cat "${PLEAMAR_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/pleamar}/build.conf" 2>/dev/null; true' }, function(out)
        for line in tostring(out or ""):gmatch("[^\n]+") do
            local m = line:match("^%s*model%s+(%S+)")
            if m then chosen = m end
        end
        text["b.chosen"] = chosen ~= "" and chosen or "the strongest there is"
        if done then done() end
    end)
end

local function show_models()
    local out = {}
    for k, name in ipairs(models) do
        out[k] = { name = (NAMES_OF[name:match("^([^/]+)/") or ""] or name:match("^([^/]+)/") or "") .. " · " .. name:gsub("^[^/]+/", ""), on = name == chosen and 1 or 0 }
        if k >= 24 then break end
    end
    model["b.models"] = out
end

local function say(m)
    local line = json.encode(m) .. "\n"
    if engine and engine_ready then write(engine, line) else waiting[#waiting + 1] = line end
end

local function on_engine(m)
    if m.type == "models" then
        engine_ready = true
        fact["b.asked"] = true
        local signed = {}
        for _, p in ipairs(m.signed or {}) do signed[p] = true end
        for k, p in ipairs(PROVIDERS) do fact["b.signed." .. (k - 1)] = signed[p] == true end
        models = m.models or {}
        show_models()
        if m.reason then text["b.status"] = "The engine says: " .. tostring(m.reason) end
    elseif m.type == "login_url" then
        text["b.status"] = m.code and ("In your browser: enter the code " .. tostring(m.code) .. " to finish signing in") or "Finish signing in in your browser"
        run("xdg-open", { tostring(m.url) }, function() end)
    elseif m.type == "login_done" then
        text["b.status"] = m.ok and "Signed in" or ("The sign-in did not finish: " .. tostring(m.reason or "?"))
        say({ type = "models" })
    end
end

local starting = false
local function start_engine()
    if engine or starting then return end
    starting = true
    engine_ready = false
    text["b.status"] = "Waking the engine up (the first time it is fetched: a minute)…"
    run("sh", { "-c", 'cd "$0/agent" && { [ -d node_modules/@earendil-works/pi-coding-agent ] && [ ! package-lock.json -nt node_modules/.package-lock.json ]; } || { command -v npm >/dev/null || { echo "npm is missing: install Node.js (and npm) to build with AI"; exit 3; }; npm ci --ignore-scripts --no-audit --no-fund >/dev/null 2>&1 || { echo "the engine could not be installed (npm ci)"; exit 4; }; }', BUILD }, function(out, code)
        starting = false
        if code ~= 0 then
            waiting = {}
            fact["b.asked"] = true
            text["b.status"] = tostring(out or ""):gsub("%s+$", "")
            return
        end
        if text["b.status"]:sub(1, 6) == "Waking" then text["b.status"] = "" end
        local ok, id = pcall(spawn, "sh", { BUILD .. "/agent/sandbox" }, function(line)
            if line:sub(1, 1) ~= "{" then
                if line:sub(1, 7) == "build: " then text["b.status"] = line:sub(8) end
                return
            end
            local good, m = pcall(json.decode, line)
            if good and type(m) == "table" then on_engine(m) end
        end, function() engine, engine_ready = nil, false end, { stdin = "open", errors = true })
        if not ok then text["b.status"] = "The engine could not start: " .. tostring(id) return end
        engine = id
        engine_ready = true
        for _, l in ipairs(waiting) do write(engine, l) end
        waiting = {}
        say({ type = "models" })
    end, { errors = true })
end

--  Opened by a click (`go`) or from outside (`--say … "fact page 3"`): the
--  logic's own change to a fact does not reach its `fact:` handlers.
open_build = function()
    conf(function() start_engine(); if engine then say({ type = "models" }) end end)
end
on("fact:page", function(p)
    if p == 3 then open_build() end
    if p == 4 then dock_look() end
end)
on("b_login", function(k)
    local p = PROVIDERS[(tonumber(k) or -1) + 1]
    if not p then return end
    if not engine and not starting then
        start_engine()
        if not starting and not engine then return end
    end
    text["b.status"] = "Opening " .. NAMES_OF[p] .. "'s sign-in in your browser…"
    say({ type = "login", provider = p })
end)
on("b_logout", function(k)
    local p = PROVIDERS[(tonumber(k) or -1) + 1]
    if p then say({ type = "logout", provider = p }) end
end)
on("b_pick", function(i)
    local name = models[(tonumber(i) or -1) + 1]
    if not name then return end
    chosen = name
    show_models()
    text["b.chosen"] = name
    run("sh", { "-c", 'd="${PLEAMAR_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/pleamar}"; mkdir -p "$d" && printf "# pleamar-wm build: the model that builds (the control centre writes it)\\nmodel %s\\n" "$0" > "$d/build.conf"', name }, function(_, code)
        text.toast = code == 0 and (name .. " builds from now on") or "Not saved"
        fact.toasted = fact.toasted + 1
    end)
end)
on("b_start", function()
    run("pleamar-wm", { "build", "on" }, function() end)
end)
