-- ── the mode, and drawing what you want ──
--  On, every monitor shows it until you leave. The pen draws freely —as many
--  strokes as you like—, the box draws a box; Done takes what is drawn: the
--  region it covers becomes the creation's place, and two pictures go with
--  your first message, the drawing itself and what was under it, so the
--  builder sees the shape you meant.
return function(shared)
    local creations = shared.creations
    local STATE = shared.STATE

    local points = {}        -- { x, y, px, py, join, s }
    local last = nil         -- the stroke's last point
    local pushing = false
    local function push()
        if pushing then return end
        pushing = true
        after(30, function()
            pushing = false
            model.pts = points
        end)
    end
    local function clear()
        points, last = {}, nil
        fact.strokes = 0
        fact.boxed = false
        fact.drawing = false
        model.pts = points
    end
    local old_ids = {}
    local function show_olds()
        creations.olds(function(list)
            --  Asked while leaving: nothing to outline any more.
            if not fact.on then return end
            local out = {}
            old_ids = {}
            for _, item in ipairs(list) do
                old_ids[#old_ids + 1] = item.id
                local r = item.region
                out[#out + 1] = { x = r.x, y = r.y, w = r.w, h = r.h, s = r.monitor_index or 0, name = tostring(item.name or item.id) }
                if #out >= 16 then break end
            end
            model.olds = out
        end)
    end

    -- ── how it looks, and how it starts: build-theme.conf (and build.conf) ──
    --  `key value` or `key = value`, a line each, # for a remark:
    --    font = Inter        the family of every word (the default one, as ever, if none)
    --    corners = 1.2       every corner that much rounder (0.5 to 2)
    --    glass = on          the bar and the panel of glass
    --    start = panel       Build asks what to build first, as a search; `draw` as ever
    --  `start` from build.conf has the last word: it is the user's, the theme a desktop's.
    local start_mode = "draw"
    local function read_theme(done)
        local dir = '"${PLEAMAR_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/pleamar}"'
        run("sh", { "-c", "cat " .. dir .. "/build-theme.conf 2>/dev/null; echo '\n#build.conf'; cat " .. dir .. "/build.conf 2>/dev/null; true" }, function(out)
            local font, r, glass, mode = "", 1, false, "draw"
            local own = false
            for line in tostring(out or ""):gmatch("[^\n]+") do
                if line:match("^#build%.conf") then own = true end
                local k, v = line:gsub("#.*$", ""):match("^%s*([%w_]+)%s*[=%s]%s*(.-)%s*$")
                if k and v and v ~= "" then
                    v = v:gsub('^"(.*)"$', "%1")
                    if not own and k == "font" then font = v
                    elseif not own and k == "corners" then r = math.min(math.max(tonumber(v) or 1, 0.5), 2)
                    elseif not own and k == "glass" then glass = (v == "on" or v == "yes" or v == "true" or v == "1")
                    elseif k == "start" then mode = (v == "panel") and "panel" or "draw" end
                end
            end
            text["ui.font"] = font
            fact["ui.r"] = r
            fact["ui.glass"] = glass
            start_mode = mode
            if done then done() end
        end)
    end

    -- ── on and off ──
    local function turn_on(ask)
        fact.on = true
        fact.tool = "pen"
        fact.asking = (ask == true or start_mode == "panel") and not fact.building
        creations.prepare()
        show_olds()
    end
    local function turn_off()
        if fact.building then creations.leave() end
        clear()
        fact.sketch = false
        model.olds = {}
        fact.view = "chat"
        fact.asking = false
        text["ask.input"] = ""
        fact.on = false
        --  Nothing left to do here: the scene goes (what you kept runs on its own).
        after(700, function()
            if not fact.on then run("pleamar", { "--say", "build", "quit" }, function() end) end
        end)
    end
    on("toggle", function() if fact.on then turn_off() else turn_on() end end)
    on("leave", turn_off)
    --  The panel to the other side of the monitor (the ⇄ in its head).
    on("flip_side", function() fact.side = fact.side == "left" and "right" or "left" end)
    on("start", function() if not fact.on then turn_on() end end)
    on("ask", function()
        if not fact.on then turn_on(true) elseif not fact.building then fact.asking = true end
    end)
    on("ask_draw", function()
        fact.asking = false
        text["ask.input"] = ""
    end)
    --  Started by `pleamar-wm build` (or `build ask`): on at once, as its theme says.
    local how = sys.ask("env", "BUILD_ON") or ""
    read_theme(function()
        if how == "1" then turn_on() elseif how == "ask" then turn_on(true) end
    end)

    -- ── the pen ──
    on("fact:pt", function(v)
        if not fact.drawing or fact.tool ~= "pen" then return end
        v = tonumber(v) or 0
        local x, y = math.floor(v / 8192), v % 8192
        local s = fact.draw_screen
        if last and last.s == s then
            local dx, dy = x - last.x, y - last.y
            if dx * dx + dy * dy < 16 then return end
        end
        if #points >= 600 then return end
        local p = { x = x, y = y, px = last and last.x or x, py = last and last.y or y, join = last and 1 or 0, s = s }
        points[#points + 1] = p
        last = p
        push()
    end)
    on("stroke_end", function()
        if last then fact.strokes = (fact.strokes or 0) + 1 end
        last = nil
    end)
    --  Strokes on another monitor than the first: they start the drawing anew.
    on("fact:draw_screen", function(s)
        local first = points[1]
        if first and first.s ~= s then
            points = {}
            last = nil
            fact.strokes = 0
            push()
        end
    end)
    -- ── the box ──
    on("box_end", function()
        if math.abs(fact.x1 - fact.x0) > 8 and math.abs(fact.y1 - fact.y0) > 8 then
            points, last = {}, nil
            model.pts = points
            fact.strokes = 0
            fact.boxed = true
        end
    end)
    on("clear", clear)
    on("fact:tool", function(v) if v == "box" and fact.strokes > 0 then fact.boxed = false end end)

    -- ── Done ──
    local function monitors(done)
        local function parse(txt)
            local out = {}
            for block in (tostring(txt or "") .. "\n\n"):gmatch("(Monitor .-)\n\n") do
                local name = block:match("^Monitor (%S+)")
                local w, h, x, y = block:match("(%d+)x(%d+)@[%d%.]+ at (%-?%d+)x(%-?%d+)")
                local scale = tonumber(block:match("scale: ([%d%.]+)")) or 1
                if name and w then
                    out[#out + 1] = { name = name, index = tonumber(block:match("%(ID (%d+)%)") or "") or #out, x = tonumber(x), y = tonumber(y),
                                      w = math.floor(tonumber(w) / scale + 0.5), h = math.floor(tonumber(h) / scale + 0.5) }
                end
            end
            done(out)
        end
        if (sys.ask("env", "HYPRLAND_INSTANCE_SIGNATURE") or "") ~= "" then run("hyprctl", { "monitors" }, parse) else run("pleamar-wm", { "hyprctl", "monitors" }, parse) end
    end
    on("done", function()
        if fact.building then return end
        local s = fact.draw_screen
        local x0, y0, x1, y1
        if fact.boxed then
            x0, y0 = math.min(fact.x0, fact.x1), math.min(fact.y0, fact.y1)
            x1, y1 = math.max(fact.x0, fact.x1), math.max(fact.y0, fact.y1)
        else
            for _, p in ipairs(points) do
                if p.s == s then
                    x0, y0 = math.min(x0 or p.x, p.x), math.min(y0 or p.y, p.y)
                    x1, y1 = math.max(x1 or p.x, p.x), math.max(y1 or p.y, p.y)
                end
            end
        end
        if not x0 then return end
        --  A line is a thin shape: at least something to put a thing in.
        if y1 - y0 < 24 then local c = (y0 + y1) / 2; y0, y1 = c - 12, c + 12 end
        if x1 - x0 < 24 then local c = (x0 + x1) / 2; x0, x1 = c - 12, c + 12 end
        x0, y0 = math.max(0, math.floor(x0)), math.max(0, math.floor(y0))
        local w, h = math.floor(x1 - x0), math.floor(y1 - y0)
        local drawn = points
        local boxed = fact.boxed
        monitors(function(list)
            local m = nil
            for _, l in ipairs(list) do if l.index == s then m = l end end
            m = m or list[s + 1] or list[1]
            if not m then return end
            --  The drawing as a picture (only the strokes), and what is under it.
            local pad = 24
            local cx, cy = math.max(0, x0 - pad), math.max(0, y0 - pad)
            local cw, ch = math.min(m.w - cx, w + 2 * pad), math.min(m.h - cy, h + 2 * pad)
            local stamp = os.time()
            local sketch = STATE .. "/refs/drawing-" .. stamp .. ".png"
            local under = STATE .. "/refs/under-" .. stamp .. ".png"
            local draws = {}
            if boxed then
                draws[#draws + 1] = string.format("roundrectangle %d,%d %d,%d 12,12", x0 - cx, y0 - cy, x0 + w - cx, y0 + h - cy)
            else
                local line = {}
                local function flush() if #line > 1 then draws[#draws + 1] = "polyline " .. table.concat(line, " ") end; line = {} end
                for _, p in ipairs(drawn) do
                    if p.s == s then
                        if p.join < 0.5 then flush() end
                        line[#line + 1] = string.format("%d,%d", p.x - cx, p.y - cy)
                    end
                end
                flush()
            end
            local argv = { "-size", string.format("%dx%d", cw, ch), "xc:#0c1413", "-fill", "none", "-stroke", "#7fe0c4", "-strokewidth", "5", "-draw", "stroke-linecap round stroke-linejoin round" }
            for _, d in ipairs(draws) do argv[#argv + 1] = "-draw"; argv[#argv + 1] = d end
            argv[#argv + 1] = sketch
            run("mkdir", { "-p", STATE .. "/refs" }, function()
                run("magick", argv, function(_, c1)
                    run("grim", { "-g", string.format("%d,%d %dx%d", m.x + cx, m.y + cy, cw, ch), under }, function(_, c2)
                        local pictures = {}
                        if c1 == 0 then pictures[#pictures + 1] = { path = sketch, what = "sketch" } end
                        if c2 == 0 then pictures[#pictures + 1] = { path = under, what = "under" } end
                        --  The drawing stays, as its sketch, until it is built.
                        fact.sketch = true
                        creations.from_drawing(m, x0, y0, w, h, pictures)
                    end, { errors = true })
                end, { errors = true })
            end)
        end)
    end)

    -- ── «panel first»: what was asked for, built in the middle of its monitor ──
    --  A widget's place, which you can move or redraw later; the words go to
    --  the builder as your first message, as if written in the panel.
    on("ask_send", function()
        local words = tostring(text["ask.input"] or "")
        if words:match("^%s*$") or fact.building then return end
        local s = fact.ask_screen or 0
        monitors(function(list)
            local m = nil
            for _, l in ipairs(list) do if l.index == s then m = l end end
            m = m or list[s + 1] or list[1]
            if not m then return end
            local w, h = math.min(420, math.floor(m.w / 3)), math.min(260, math.floor(m.h / 3))
            local x, y = math.floor((m.w - w) / 2), math.floor((m.h - h) / 2)
            fact.asking = false
            text["ask.input"] = ""
            clear()
            fact.draw_screen = m.index
            fact.x0, fact.y0, fact.x1, fact.y1 = x, y, x + w, y + h
            fact.boxed = true
            fact.sketch = true
            creations.from_drawing(m, x, y, w, h, {})
            if shared.say then shared.say(words) end
        end)
    end)

    -- ── a piece of the screen, as a reference ──
    on("shot_end", function()
        fact.shooting = false
        local x0, y0 = math.floor(math.min(fact.sx0, fact.sx1)), math.floor(math.min(fact.sy0, fact.sy1))
        local w, h = math.floor(math.abs(fact.sx1 - fact.sx0)), math.floor(math.abs(fact.sy1 - fact.sy0))
        if w < 8 or h < 8 then return end
        local s = fact.shot_screen
        monitors(function(list)
            local m = nil
            for _, l in ipairs(list) do if l.index == s then m = l end end
            m = m or list[s + 1] or list[1]
            if not m then return end
            local path = STATE .. "/refs/shot-" .. os.time() .. ".png"
            --  A moment for the dim to go before the picture is taken.
            after(250, function()
                run("mkdir", { "-p", STATE .. "/refs" }, function()
                    run("grim", { "-g", string.format("%d,%d %dx%d", m.x + x0, m.y + y0, w, h), path }, function(_, code)
                        if code == 0 then creations.add_picture(path) end
                    end, { errors = true })
                end)
            end)
        end)
    end)

    -- ── what you built, on the screen ──
    on("open_old", function(i)
        creations.open_id(old_ids[(tonumber(i) or -1) + 1])
    end)
    --  A creation left (kept, thrown or as it is): the pen again, and what you
    --  built drawn on the screen.
    creations.finished = function()
        clear()
        fact.sketch = false
        fact.tool = "pen"
        show_olds()
    end
end
