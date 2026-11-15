-- ── creations: what is built, kept, shared and replaced ───────────
--  Each creation is a folder of this scene's (creations/<id>/) with its
--  scene, its logic, its icon, its versions and creation.json (what it is,
--  where, what it was allowed, its conversation). Only this writes there:
--  the model proposes, and every file goes through the checks below.
--
--  What a creation may ask the system for is decided here by fixed rules,
--  never by the model: what is harmless goes through, what is dangerous is
--  refused, and anything else is asked of you on a card, in plain words.
--
--  What you keep runs without this scene: pleamar-wm starts it with the
--  session (~/.config/pleamar/creations.autostart), and hides what it
--  replaced whenever that appears (~/.config/pleamar/creations.hide).
return function(shared)
    local home_dir = shared.home_dir
    local FOLDER = tostring(sys.ask("files.folder") or "")
    --  Where the engine can see a picture (its sandbox's /state).
    local STATE = shared.STATE
    local on_hyprland = (sys.ask("env", "HYPRLAND_INSTANCE_SIGNATURE") or "") ~= ""

    local current = nil      -- the creation being built: its record (creation.json)
    local surface_box        -- (below)
    local scene_id = nil     -- its scene, running (spawn's id)
    --  What its scene says (its logic's errors, its log(), its permissions):
    --  the last lines, for the model to read. Without them it could not tell
    --  its own mistake from pleamar's, and blamed pleamar.
    local said = {}
    local function heard(line)
        line = tostring(line)
        if line:match("^timing") or line:match("^cycle") or line:match("^render ·") or line:match("^text ") then return end
        said[#said + 1] = line
        if #said > 60 then table.remove(said, 1) end
    end
    local function lately()
        if #said == 0 then return "" end
        local from = math.max(1, #said - 30)
        return "\n\nWhat the creation has said lately (its log: logic errors, log(), permissions):\n" .. table.concat(said, "\n", from)
    end
    local skill = nil        -- pleamar's guide to writing scenes, read once
    local desktop = nil      -- and its guide to the desktop
    local guide = nil        -- the desktop's own guide for what is built on it ($PLEAMAR_CONFIG/build-guide.md), if it has one
    local source = ""        -- pleamar's source, where there is one
    local pleamar_path = "pleamar" -- the program by its whole path, for an app's .desktop
    local marea_dir = ""     -- Marea's folder, if she is here (her code is readable)
    local chat = shared.engine
    local api = {}

    fact.building = false
    text["build.name"] = ""

    local function where(id) return "creations/" .. id end
    --  The model calls them scene.plm and scene.luau; on disk they carry the
    --  creation's id, because pleamar names a scene's socket and its files
    --  folder after its file: two kept creations both called «scene» would
    --  answer for each other and share their things.
    local function disk(id, file)
        if file == "scene.plm" then return id .. ".plm" end
        if file == "scene.luau" then return id .. ".luau" end
        return file
    end
    local function path_of(id, file) return FOLDER .. "/" .. where(id) .. "/" .. disk(id, file) end
    local function record_save()
        if current then pcall(sys.call, "files.write", where(current.id) .. "/creation.json", current) end
    end
    local function read(id, file)
        local ok, t = pcall(sys.ask, "files.read", where(id) .. "/" .. disk(id, file))
        if ok and type(t) == "string" then return t end
        return nil
    end
    local function index()
        local ok, t = pcall(sys.ask, "files.read", "creations/index.json", "json")
        return (ok and type(t) == "table") and t or { list = {} }
    end
    local function index_save(ix) pcall(sys.call, "files.write", "creations/index.json", ix) end

    -- ── what a creation may ask the system for ──
    --  Programs, by what they do. Harmless ones are allowed without asking;
    --  dangerous ones are refused (the model is told to do without them);
    --  every shell and interpreter, and anything unknown, is asked, because
    --  through them anything can be done.
    local SAFE_RUN = {
        date = "read the date and time", playerctl = "control the music that is playing",
        brightnessctl = "change the screen's brightness", ["notify-send"] = "show notifications",
        wpctl = "change the volume", pactl = "change the volume and the sound devices",
        uptime = "see how long the computer has been on", uname = "see the system's name",
        hostname = "see the computer's name", whoami = "see your user name",
    }
    local DANGER_RUN = {
        rm = "delete files", rmdir = "delete folders", shred = "destroy files", unlink = "delete files",
        dd = "write straight onto disks", truncate = "empty files", wipefs = "erase disks",
        fdisk = "change the disks' partitions", parted = "change the disks' partitions",
        mount = "mount disks", umount = "unmount disks",
        sudo = "do things as the administrator", pkexec = "do things as the administrator",
        su = "do things as another user", doas = "do things as the administrator",
        chmod = "change who can open files", chown = "change who owns files", chattr = "change how files can be changed",
        systemctl = "start, stop and change system services", loginctl = "end sessions",
        shutdown = "turn the computer off", reboot = "restart the computer", poweroff = "turn the computer off", halt = "stop the computer",
        passwd = "change passwords", useradd = "create users", usermod = "change users", userdel = "delete users",
        crontab = "run things on its own at times", iptables = "change the firewall", nft = "change the firewall", ufw = "change the firewall",
        ssh = "connect to other computers", scp = "send files to other computers", rsync = "copy files elsewhere",
        nc = "open network connections", ncat = "open network connections", socat = "open network connections",
        pacman = "install or remove software", yay = "install or remove software", paru = "install or remove software",
        apt = "install or remove software", dnf = "install or remove software", flatpak = "install or remove software",
        mv = "move and replace files", cp = "copy over files", tee = "write files", install = "replace files",
        kill = "close programs", pkill = "close programs", killall = "close programs",
    }
    local INTERPRETERS = {
        sh = 1, bash = 1, zsh = 1, fish = 1, dash = 1, ksh = 1, busybox = 1, env = 1, xargs = 1, find = 1, awk = 1, sed = 1,
        python = 1, python3 = 1, perl = 1, ruby = 1, node = 1, lua = 1, luajit = 1, php = 1, osascript = 1,
    }
    --  Services pleamar brings: listening is harmless; acting on the system
    --  is asked; a few are not for something made in a chat.
    local SAFE_SERVICES = {
        clock = 1, ["clock.seconds"] = 1, audio = 1, media = 1, network = 1, battery = 1, brightness = 1,
        workspaces = 1, window = 1, apps = 1, tray = 1, notifications = 1, bluetooth = 1,
        files = 1, ["files.write"] = 1, ["files.remove"] = 1,
    }
    local ASK_SERVICES = {
        ["audio.*"] = "change the volume and the sound devices", ["audio.volume"] = "change the volume",
        ["media.*"] = "control the music that is playing", ["brightness.*"] = "change the screen's brightness",
        ["workspaces.*"] = "switch workspaces", ["workspaces.focus"] = "switch workspaces",
        ["window.*"] = "move and close your windows", ["apps.launch"] = "open programs",
        ["notifications.*"] = "answer and clear notifications", ["tray.*"] = "use the icons in the tray",
        ["network.*"] = "turn the network on and off, and connect to Wi-Fi",
        ["bluetooth.*"] = "connect and disconnect Bluetooth devices",
        clipboard = "read what you copy", ["clipboard.set"] = "put things in what you copy",
        env = "read the session's settings (environment)", ["session.*"] = "log out, restart or turn off the computer",
    }
    local DANGER_SERVICES = { auth = "check your password" }

    --  What a scene's permissions {} blocks ask for: programs and services.
    local function asks_of(content)
        local runs, services = {}, {}
        for block in tostring(content):gmatch("permissions%s*{(.-)}") do
            for part in (block .. ";"):gmatch("([^;]*);") do
                local key, values = part:match("^%s*(%a+)%s*:(.*)$")
                if key then
                    for v in values:gmatch('"([^"]*)"') do
                        if key == "run" then runs[#runs + 1] = v elseif key == "services" then services[#services + 1] = v end
                    end
                end
            end
        end
        return runs, services
    end
    local function has(list, v)
        for _, x in ipairs(list or {}) do if x == v then return true end end
        return false
    end
    --  Of what this content asks for, what is new: refused (a reason each),
    --  to ask (in plain words), and harmless.
    local function judge(content)
        local runs, services = asks_of(content)
        local approved = current.approved
        local refused, ask, fine = {}, {}, { run = {}, services = {} }
        for _, r in ipairs(runs) do
            if not has(approved.run, r) then
                --  The program is the first word; what follows are the only
                --  arguments it may start with (pleamar enforces it).
                local prog = r:match("^(%S+)") or r
                local scripted = r:find("%s") ~= nil
                local name = prog:match("([^/]+)$") or prog
                if DANGER_RUN[name] or name:match("^mkfs") then
                    refused[#refused + 1] = string.format(tr("run «%s», which can %s"), r, tr(DANGER_RUN[name] or "format disks"))
                --  By its whole path it may be anything with a known name:
                --  asked, never taken as harmless.
                elseif INTERPRETERS[name] and scripted then
                    ask[#ask + 1] = { run = r, words = string.format(tr("run «%s»: only that script, nothing else"), r) }
                elseif prog:find("/", 1, true) or scripted then
                    ask[#ask + 1] = { run = r, words = string.format(tr("run «%s», a program it does not know"), r) }
                elseif SAFE_RUN[name] then
                    table.insert(fine.run, r)
                elseif INTERPRETERS[name] then
                    ask[#ask + 1] = { run = r, words = string.format(tr("run «%s»: a program that runs other programs. Through it, this creation could do anything on your computer"), r) }
                else
                    ask[#ask + 1] = { run = r, words = string.format(tr("run «%s», a program it does not know"), r) }
                end
            end
        end
        for _, s in ipairs(services) do
            if not has(approved.services, s) then
                if DANGER_SERVICES[s] then
                    refused[#refused + 1] = tr(DANGER_SERVICES[s])
                elseif SAFE_SERVICES[s] then
                    table.insert(fine.services, s)
                else
                    ask[#ask + 1] = { service = s, words = ASK_SERVICES[s] and tr(ASK_SERVICES[s]) or string.format(tr("use «%s» of the system"), s) }
                end
            end
        end
        return refused, ask, fine
    end
    local function approve(ask, fine)
        for _, r in ipairs(fine.run) do if not has(current.approved.run, r) then table.insert(current.approved.run, r) end end
        for _, s in ipairs(fine.services) do if not has(current.approved.services, s) then table.insert(current.approved.services, s) end end
        for _, a in ipairs(ask or {}) do
            if a.run and not has(current.approved.run, a.run) then table.insert(current.approved.run, a.run) end
            if a.service and not has(current.approved.services, a.service) then table.insert(current.approved.services, a.service) end
        end
    end

    -- ── looking around the computer, as a developer would ──
    --  To find where some data lives or how another app gets it. Reading is
    --  done at once; a program that is not a known reader is asked of you
    --  with its whole command; a dangerous one is refused; and files that may
    --  hold passwords or keys are asked even to read.
    local READERS = {
        ls = 1, cat = 1, head = 1, tail = 1, grep = 1, rg = 1, fd = 1, find = 1, which = 1, file = 1, stat = 1,
        wc = 1, du = 1, df = 1, ps = 1, uname = 1, date = 1, whoami = 1, free = 1, uptime = 1, lsblk = 1,
        lscpu = 1, tree = 1, jq = 1, realpath = 1, readlink = 1, basename = 1, dirname = 1, sort = 1, uniq = 1,
    }
    local FIND_ACTS = { ["-delete"] = 1, ["-exec"] = 1, ["-execdir"] = 1, ["-ok"] = 1, ["-okdir"] = 1, ["-fprint"] = 1, ["-fprintf"] = 1, ["-fls"] = 1 }
    local SECRET_WORDS = { "%.ssh", "%.gnupg", "password", "keyring", "auth%.json", "%.netrc", "credential", "%.env",
        "cookies", "logins%.json", "key4%.db", "login data", "token", "secret", "/etc/shadow", "%.pki", "wallet" }
    local function secretive(words)
        for _, w in ipairs(words) do
            local low = tostring(w):lower()
            for _, s in ipairs(SECRET_WORDS) do
                if low:find(s) then return true end
            end
        end
        return false
    end
    local function home_path(p)
        p = tostring(p or "")
        if p:sub(1, 2) == "~/" then return home_dir .. p:sub(2) end
        if p == "~" then return home_dir end
        return p
    end
    --  What to do with one: nil now, { ask, detail } or { refuse, note }.
    local function judge_system(tool, args)
        if tool == "system_read" then
            if secretive({ args.path }) then
                return { ask = string.format(tr("The builder wants to read «%s», a file that may hold passwords or keys."), tostring(args.path)) .. "\n" .. tostring(args.why or ""), detail = tr("Reading a private file") }
            end
            return nil
        end
        local program = tostring(args.program or "")
        local name = program:match("([^/]+)$") or program
        local list = type(args.args) == "table" and args.args or {}
        local command = program .. (#list > 0 and (" " .. table.concat(list, " ")) or "")
        if DANGER_RUN[name] or name:match("^mkfs") then
            return { refuse = "Refused: «" .. name .. "» can " .. (DANGER_RUN[name] or "format disks") .. ". Find it out another way.",
                      note = string.format(tr("Not allowed: run «%s», which can %s."), name, tr(DANGER_RUN[name] or "format disks")) }
        end
        if secretive(list) then
            return { ask = tr("The builder wants to run on your computer, touching files that may hold passwords or keys:") .. "\n" .. command .. "\n" .. tostring(args.why or ""), detail = tr("A private file") }
        end
        local acts = false
        if name == "find" then for _, a in ipairs(list) do if FIND_ACTS[a] then acts = true end end end
        if READERS[name] and not acts then return nil end
        --  curl that only reads a page and prints it (a logo's SVG, a page's
        --  data): flags among -s -S -L -f, and https addresses. Saving to a
        --  file, sending data or anything else is asked.
        if name == "curl" and #list > 0 then
            local only_reads = true
            for _, a in ipairs(list) do
                a = tostring(a)
                if not (a:match("^%-[sSLf]+$") or a == "--compressed" or a:match("^https://[%w%-%.]+/[%w%-%._~/%%@+=&?]*$")) then only_reads = false end
            end
            if only_reads then return nil end
        end
        local warn = INTERPRETERS[name] and ("\n" .. tr("It is a program that runs other programs: it can do anything on your computer.")) or ""
        return { ask = tr("The builder wants to run on your computer:") .. "\n" .. command .. warn .. "\n" .. tostring(args.why or ""), detail = tr("Running a program") }
    end
    local function system_do(tool, args, done)
        if tool == "system_read" then
            run("sh", { "-c", 'head -c 65536 -- "$0"', home_path(args.path) }, function(out, code)
                done(code == 0, code == 0 and tostring(out) or ("Could not read it: " .. tostring(out)))
            end, { errors = true })
            return
        end
        local argv = { "-c", 'cd "$HOME" && timeout 20 "$0" "$@" 2>&1 | head -c 24576', home_path(args.program) }
        for _, a in ipairs(type(args.args) == "table" and args.args or {}) do argv[#argv + 1] = tostring(a) end
        run("sh", argv, function(out, code)
            done(true, "Exit " .. tostring(code) .. ":\n" .. tostring(out))
        end, { errors = true })
    end

    -- ── its scene, on the screen ──
    local function check(done)
        if not current then done(false, "Nothing is being built.") return end
        run("pleamar", { "--check", path_of(current.id, "scene.plm") }, function(out, code)
            done(code == 0, (tostring(out or ""):gsub("%s+$", "")) .. lately())
        end, { errors = true })
    end
    local when_stopped = nil   -- what waits for its scene to have gone
    local function start_scene()
        if scene_id or not current then return end
        local ok, id = pcall(spawn, "pleamar", { "--scene", path_of(current.id, "scene.plm"), "--no-hud" }, function(line)
            log("build · " .. tostring(line))
            heard(line)
        end, function(_, code)
            log("build · its scene ended", code)
            scene_id = nil
            local then_ = when_stopped
            when_stopped = nil
            if then_ then then_() end
        end, { errors = true })
        if ok then scene_id = id else log("build · its scene could not start: " .. tostring(id)) end
    end
    --  `done` once it has gone: a scene started again before that would
    --  find its own name still taken.
    local function stop_scene(done)
        if scene_id then
            when_stopped = done
            kill(scene_id, "term")
            scene_id = nil
        elseif done then
            done()
        end
    end

    -- ── where it was drawn ──
    --  The monitors as the compositor says them: name, place and size.
    local function monitors(done)
        local function parse(txt)
            local out = {}
            for block in (tostring(txt or "") .. "\n\n"):gmatch("(Monitor .-)\n\n") do
                local name = block:match("^Monitor (%S+)")
                local w, h, x, y = block:match("(%d+)x(%d+)@[%d%.]+ at (%-?%d+)x(%-?%d+)")
                local scale = tonumber(block:match("scale: ([%d%.]+)")) or 1
                if name and w then
                    out[#out + 1] = { name = name, index = tonumber(block:match("%(ID (%d+)%)") or ""), x = tonumber(x), y = tonumber(y), w = math.floor(tonumber(w) / scale + 0.5), h = math.floor(tonumber(h) / scale + 0.5) }
                end
            end
            done(out)
        end
        if on_hyprland then run("hyprctl", { "monitors" }, parse) else run("pleamar-wm", { "hyprctl", "monitors" }, parse) end
    end
    --  What a drawing is, by its shape and place: a strip against the top or
    --  the bottom is a bar, low and centred a dock, anything else a widget.
    --  The surface block puts it there.
    local function shape(r)
        local x, y, w, h, mw, mh = r.x, r.y, r.w, r.h, r.mw, r.mh
        local screens = string.format('screens: "%s"', r.monitor)
        if w >= mw * 0.6 and y <= 16 then
            return "bar", string.format("surface { size: full, %d; anchor: top; reserve: %d; %s; keyboard: none }", h, h, screens), mw, h
        elseif w >= mw * 0.6 and y + h >= mh - 16 then
            return "bar", string.format("surface { size: full, %d; anchor: bottom; reserve: %d; %s; keyboard: none }", h, h, screens), mw, h
        elseif y + h >= mh - 60 and math.abs(x + w / 2 - mw / 2) < mw * 0.12 then
            return "dock", string.format("surface { size: %d, %d; anchor: bottom; margin: 0, 0, %d, 0; %s; keyboard: none }", w, h, math.max(0, mh - y - h), screens), w, h
        end
        --  On the desktop, under the windows (and under her chat while you build).
        return "widget", string.format("surface { size: %d, %d; anchor: top_left; margin: %d, 0, 0, %d; level: bottom; %s; keyboard: none }", w, h, y, x, screens), w, h
    end
    --  Where a scene's surface block puts it on its monitor: size, anchor,
    --  margin. nil if it cannot be told (a size from a fact).
    surface_box = function(content, r)
        local block = tostring(content or ""):match("surface%s*{(.-)}")
        if not block then return nil end
        local w, h = block:match("size:%s*([%w]+)%s*,%s*([%w]+)")
        w = w == "full" and r.mw or tonumber(w)
        h = h == "full" and r.mh or tonumber(h)
        if not (w and h) then return nil end
        local anchor = block:match("anchor:%s*([%a_]+)") or "center"
        local m = { 0, 0, 0, 0 }
        local ms = block:match("margin:%s*([%-%d%s,]+)")
        if ms then
            local list = {}
            for n in ms:gmatch("%-?%d+") do list[#list + 1] = tonumber(n) end
            if #list == 1 then m = { list[1], list[1], list[1], list[1] } elseif #list >= 4 then m = list end
        end
        local x, y = (r.mw - w) / 2, (r.mh - h) / 2
        if anchor:find("left") then x = m[4] elseif anchor:find("right") then x = r.mw - w - m[2] end
        if anchor:find("top") then y = m[1] elseif anchor:find("bottom") then y = r.mh - h - m[3] end
        return x, y, w, h
    end
    --  Nothing of its own to show yet: build mode shows the sketch there.
    local function placeholder(name, surface, w, h)
        return string.format([[language 0.1
// Being built: «%s». It is written again as it grows.
scene Creation {
    %s
    box { from: 0, 0; size: %d, %d; opacity: 0%% }
}
]], name, surface, w, h)
    end

    -- ── apps: a window of its own, in the finder and the dock ──
    --  A creation whose scene is `kind: window` is an app: kept, it gets a
    --  .desktop file (its name, its icon.svg, how to open it) and opens from
    --  the finder, the dock and any launcher instead of starting with her.
    local function app_id_of(id) return "build." .. id end
    local function is_window(content) return tostring(content or ""):match("kind:%s*window") ~= nil end
    local function shape_words(content)
        local block = tostring(content or ""):match("surface%s*{(.-)}") or ""
        return (block:match("kind:%s*(%a+)") or "panel") .. "|" .. (block:match("screens:%s*([^;}]+)") or "") .. "|" .. (block:match("keyboard:%s*([^;}]+)") or "")
    end
    local data_home = sys.ask("env", "XDG_DATA_HOME")
    if data_home == nil or data_home == "" then data_home = home_dir .. "/.local/share" end
    local APPS = data_home .. "/applications"
    local function desktop_file(id) return APPS .. "/" .. app_id_of(id) .. ".desktop" end
    local function install_app(record)
        local icon = read(record.id, "icon.svg") and path_of(record.id, "icon.svg") or "applications-other"
        local name = tostring(record.name):gsub("[\r\n]", " ")
        local entry = table.concat({
            "[Desktop Entry]", "Type=Application", "Name=" .. name,
            "Comment=" .. tr("Made with pleamar-wm's Build"),
            "Exec=" .. pleamar_path .. " --scene " .. path_of(record.id, "scene.plm") .. " --no-hud",
            "Icon=" .. icon, "StartupWMClass=" .. app_id_of(record.id),
            "Categories=Utility;", "Terminal=false", "",
        }, "\n")
        run("sh", { "-c", 'mkdir -p "$(dirname "$0")" && cat > "$0"', desktop_file(record.id) }, function(_, code)
            log("build · " .. (code == 0 and "installed" or "could not install") .. " the app " .. record.id)
        end, { input = entry })
    end

    -- ── a key of its own ──
    --  «Open it with Super+U»: a line in pleamar-wm's keys.conf (read again
    --  at once), marked with the creation's id so it goes when it goes. Only
    --  for the creation itself: an order to its scene, or opening its app.
    local config_dir = sys.ask("env", "PLEAMAR_CONFIG")
    if config_dir == nil or config_dir == "" then
        local xdg = sys.ask("env", "XDG_CONFIG_HOME")
        config_dir = ((xdg and xdg ~= "") and xdg or (home_dir .. "/.config")) .. "/pleamar"
    end
    local KEYS = config_dir .. "/keys.conf"
    local function key_command(record, order)
        if order == "open" then return pleamar_path .. " --scene " .. path_of(record.id, "scene.plm") .. " --no-hud" end
        return pleamar_path .. " --say " .. record.id .. " '" .. order:gsub("'", "") .. "'"
    end
    local function bind_key(record, keys, order, done)
        local line = string.format("bind %s launch %s  # creation %s", keys, key_command(record, order), record.id)
        --  A keys.conf of yours that did not exist starts with `defaults`, so
        --  pleamar-wm's own keys stay.
        run("sh", { "-c", '[ -f "$0" ] || { mkdir -p "$(dirname "$0")"; printf "defaults\n" > "$0"; }; printf "%s\n" "$1" >> "$0"', KEYS, line }, function(_, code)
            done(code == 0, code == 0 and ("Bound: " .. keys .. " → " .. order .. ". pleamar-wm reads it at once (on another desktop, its own keys are needed).") or "Could not write keys.conf.")
        end, { errors = true })
    end
    local function unbind_keys(id)
        run("sh", { "-c", '[ -f "$0" ] && grep -q -F "# creation $1" "$0" && grep -v -F "# creation $1" "$0" > "$0.new" && mv "$0.new" "$0"; true', KEYS, id })
    end

    -- ── what is already there: replaced, never deleted ──
    --  pleamar-wm says which programs' surfaces are on the desktop (a bar of
    --  waybar's, a shell's panel). One under what you draw is hidden while you
    --  build; kept, yours takes its place and it stays hidden —found again and
    --  hidden whenever it comes back, after a login too—; thrown away, it
    --  comes back. Its program and your autostart are never touched.
    local function layers(done)
        run("pleamar-wm", { "agent", "layers" }, function(out, code)
            local list = {}
            if code == 0 then
                for line in tostring(out or ""):gmatch("[^\n]+") do
                    local id, who, ns, w, h, x, y, mon, level = line:match("^%s*(%d+)%s+(.-)%s+«(.-)»%s+(%d+)x(%d+) at (%-?%d+),(%-?%d+)%s+monitor (%d+)%s+(%a+)")
                    if id then
                        list[#list + 1] = { id = id, who = who, ns = ns, w = tonumber(w), h = tonumber(h), x = tonumber(x), y = tonumber(y),
                                            monitor = tonumber(mon), level = level, hidden = line:find("· hidden", 1, true) ~= nil }
                    end
                end
            end
            done(list)
        end, { errors = true })
    end
    local function same(a, b) return a.who == b.who and a.ns == b.ns end
    local function set_hidden(identity, hide)
        layers(function(list)
            for _, l in ipairs(list) do
                if same(l, identity) and l.hidden ~= hide then
                    run("pleamar-wm", { "agent", hide and "hide" or "show", l.id }, function() end, { errors = true })
                end
            end
        end)
    end
    --  Under a new creation: the biggest surface of another program's that
    --  it covers mostly, on its monitor (not hers, not a creation, not a
    --  wallpaper).
    --  What was drawn over: a surface of another program that the drawing
    --  covers for the most part (a bar drawn over, not a desktop drawn on).
    --  Only one that can be told apart from the rest of its program's: the
    --  surfaces are named by program, scene and name, and a program whose
    --  surfaces share them (Marea's, all «pleamar» and of every size) would
    --  go whole. Never Build's own.
    local function under(r, monitor_index, done)
        layers(function(list)
            local best, best_area = nil, 0
            for _, l in ipairs(list) do
                local mine = l.who:find("/creations/", 1, true) or l.who:match("build%.plm$") ~= nil or l.who:match("^pleamar%-wm") ~= nil
                local alone = true
                for _, o in ipairs(list) do
                    if same(o, l) and (o.w ~= l.w or o.h ~= l.h) then alone = false end
                end
                if not mine and alone and not l.hidden and l.level ~= "background" and l.monitor == monitor_index then
                    local ix = math.max(0, math.min(l.x + l.w, r.x + r.w) - math.max(l.x, r.x))
                    local iy = math.max(0, math.min(l.y + l.h, r.y + r.h) - math.max(l.y, r.y))
                    local area = ix * iy
                    if area > 0.5 * l.w * l.h and area > best_area then best, best_area = l, area end
                end
            end
            done(best)
        end)
    end

    -- ── beginning, and coming back to one ──
    local function context()
        if not current then return nil end
        local files = {}
        for _, f in ipairs({ "scene.plm", "scene.luau", "icon.svg" }) do
            if read(current.id, f) then files[#files + 1] = f end
        end
        --  The desktop's code, to read how it gets what it shows: the user's
        --  own shells, Marea's if she is here, pleamar's source.
        local code = { ["the user's shells and scenes"] = config_dir .. "/shells" }
        if marea_dir ~= "" then code.Marea = marea_dir end
        if source ~= "" then code["pleamar (its source, docs and examples)"] = source end
        return { name = current.name, kind = current.kind, region = current.region, files = files, skill = skill, desktop = desktop, guide = guide, code = code, app_id = app_id_of(current.id) }
    end
    local review   -- (below): what a creation from someone else asks for, on a card
    local import   -- (below): a .build.tar.gz dropped on her chat
    --  The panel: on the monitor it lives on, on the side away from it.
    local function place_panel(r)
        if not r then return end
        fact.panel_screen = r.monitor_index or 0
        fact.side = (r.x + r.w / 2 < (r.mw or 1920) / 2) and "right" or "left"
    end
    local function open(record)
        current = record
        fact.building = true
        fact.view = "chat"
        text["build.name"] = record.name
        place_panel(record.region)
        --  Where it goes, for the corners and the light over it; a new one is
        --  still its sketch until its first version is on the screen.
        local r = record.region or {}
        fact["reg.x"], fact["reg.y"], fact["reg.w"], fact["reg.h"] = r.x or 0, r.y or 0, r.w or 0, r.h or 0
        fact["reg.s"] = r.monitor_index or 0
        fact.made = #(record.versions or {}) > 0 or not fact.sketch
        --  Yours already: its changes are kept as they are made, and the head
        --  says Done instead of Keep it.
        fact["c.kept"] = record.kept == true
        if chat then chat.begin(record.history or {}, record.rows or {}) end
        --  One from someone else does not run until you allow what it asks for.
        if record.pending then review(record) else start_scene() end
    end
    --  A drawing that falls mostly on a creation already there edits it.
    local function load(id)
        local ok, t = pcall(sys.ask, "files.read", where(id) .. "/creation.json", "json")
        return (ok and type(t) == "table") and t or nil
    end
    --  A kept one runs on its own (the session started it, or keeping it
    --  did): told to leave by its own socket, which is its id.
    --  Every copy of it: one started while another ran listens as
    --  «id-PID», and told only by its id the others went on.
    local function stop_kept(id)
        run("sh", { "-c", [[d="${PLEAMAR_SOCKETS:-${XDG_RUNTIME_DIR:-/tmp}/pleamar}"; for s in "$d/$0.sock" "$d/$0"-*.sock; do [ -S "$s" ] && pleamar --say "$(basename "$s" .sock)" quit; done; true]], id }, function() end, { errors = true })
    end
    local function create(r)
        local kind, surface, w, h = shape(r)
        local ix = index()
        local n = 1
        for _, item in ipairs(ix.list) do if item.kind == kind then n += 1 end end
        local names = { bar = tr("Bar"), dock = tr("Dock"), widget = tr("Widget") }
        local id = "c" .. os.date("%Y%m%d%H%M%S")
        local record = { id = id, name = names[kind] .. " " .. n, kind = kind, region = r, made = os.time(),
                         approved = { run = {}, services = {} }, versions = {}, history = {}, rows = {}, kept = false }
        current = record
        pcall(sys.call, "files.write", where(id) .. "/" .. disk(id, "scene.plm"), placeholder(record.name, surface, w, h))
        --  Its logic, from the start, even with nothing in it: pleamar decides
        --  when a scene starts whether it has logic, and one that started
        --  without it never got the services a later version asked for.
        pcall(sys.call, "files.write", where(id) .. "/" .. disk(id, "scene.luau"), "-- Its logic: written here when it needs one.\n")
        record_save()
        table.insert(ix.list, { id = id, name = record.name, kind = kind, region = r })
        index_save(ix)
        log("build · new " .. kind .. " " .. id .. " on " .. r.monitor)
        open(record)
        if r.monitor_index then
            under(r, r.monitor_index, function(l)
                if not l or not current or current.id ~= id then return end
                current.replaces = { who = l.who, ns = l.ns }
                record_save()
                run("pleamar-wm", { "agent", "hide", l.id }, function() end, { errors = true })
                local name = (l.who:match("([^/%s]+)%.plm") or l.who:match("^(%S+)") or l.who)
                if chat then chat.note(string.format(tr("Hidden while you build: %s's «%s». Keep yours and it stays hidden; throw yours away and it comes back."), name, l.ns)) end
            end)
        end
    end
    --  Where it goes: a region of a monitor, made a creation (or the one
    --  already there, opened again).
    local function chosen(m, x, y, w, h)
        if w < 8 or h < 8 then return end
        --  Always a new one: one you built opens from its name on the screen
        --  or from What you built, not by drawing near where it once was.
        local r = { monitor = m.name, mw = m.w, mh = m.h, x = x, y = y, w = w, h = h, gx = m.x + x, gy = m.y + y, monitor_index = m.index }
        create(r)
    end
    --  From the drawing (draw.luau): a region of a monitor, and the
    --  pictures of what was drawn, which go with the first message.
    local take_picture   -- (below)
    function api.from_drawing(m, x, y, w, h, pictures)
        if current then return end
        chosen(m, x, y, w, h)
        for _, p in ipairs(pictures or {}) do take_picture(p.path, nil, p.what) end
    end
    --  Where each running creation is now, by its id: its surface on the
    --  screen, which may not be where it was drawn (one you can drag).
    local function where_now(done)
        layers(function(list)
            local now = {}
            for _, l in ipairs(list) do
                local id = l.who:match("/creations/([%w]+)/")
                if id and not l.hidden then now[id] = { x = l.x, y = l.y, w = l.w, h = l.h, monitor_index = l.monitor } end
            end
            done(now)
        end)
    end
    local function moved(region, now)
        if not now then return region end
        local r = {}
        for k, v in pairs(region or {}) do r[k] = v end
        for k, v in pairs(now) do r[k] = v end
        return r
    end
    function api.open_id(id)
        if not id or current then return end
        local record = load(id)
        if not record then return end
        where_now(function(now)
            record.region = moved(record.region, now[id])
            stop_kept(id)
            open(record)
        end)
    end
    --  What the drawing shows of what you built: where each one is now.
    function api.olds(done)
        where_now(function(now)
            local out = {}
            for _, item in ipairs(index().list) do
                if not item.thrown and item.region then
                    out[#out + 1] = { id = item.id, name = item.name, region = moved(item.region, now[item.id]) }
                end
            end
            done(out)
        end)
    end
    --  Ready before the first creation: pleamar's guides, its source, Marea.
    function api.prepare()
        if skill then return end
        run("pleamar", { "--docs", "skill" }, function(out, code) if code == 0 then skill = tostring(out) end end)
        run("pleamar", { "--docs", "desktop" }, function(out, code) if code == 0 then desktop = tostring(out) end end)
        --  A desktop made with pleamar-wm (Bahía, Calima…) can say how what is
        --  built on it should be: its own guide, beside its keys.conf. Without
        --  the file, nothing changes.
        run("sh", { "-c", 'cat "$0" 2>/dev/null', config_dir .. "/build-guide.md" }, function(out, code)
            local text = tostring(out or "")
            if code == 0 and text:match("%S") then
                guide = text
                log("build", "the desktop's guide for builders: " .. #text .. " bytes from " .. config_dir .. "/build-guide.md")
            end
        end)
        run("sh", { "-c", 'd=$(dirname "$(readlink -f "$(command -v pleamar)")"); for c in "$d/../.." "$HOME/.local/share/pleamar/src/pleamar"; do [ -f "$c/Cargo.toml" ] && { readlink -f "$c"; exit 0; }; done' }, function(out)
            source = tostring(out or ""):gsub("%s+$", "")
        end)
        run("sh", { "-c", 'm=$(command -v marea) && dirname "$(readlink -f "$m")"' }, function(out)
            marea_dir = tostring(out or ""):gsub("%s+$", "")
        end)
        run("sh", { "-c", "command -v pleamar" }, function(out)
            local p = tostring(out or ""):gsub("%s+$", "")
            if p ~= "" then pleamar_path = p end
        end)
    end

    -- ── the tools she has while building ──
    local OWN = { creation_read = 1, creation_write = 1, creation_check = 1, creation_look = 1, creation_poke = 1, creation_name = 1, creation_key = 1, pleamar_docs = 1, system_run = 1, system_read = 1 }
    local WORDS = {
        creation_read = "Reading %s", creation_write = "Writing %s", creation_check = "Checking it",
        creation_look = "Looking at it", pleamar_docs = "Reading pleamar's %s",
        system_run = "Running %s", system_read = "Reading %s", creation_poke = "Trying it: %s", creation_name = "Calling it %s", creation_key = "A key: %s",
    }
    function api.owns(tool) return OWN[tool] ~= nil end
    function api.active() return current ~= nil end
    function api.context() return context() end
    function api.said(tool, args)
        return string.format(tr(WORDS[tool] or tool), tostring(args.file or args.topic or args.program or args.path or args.keys or args.order or args.name or ""))
    end
    --  Before it is done: nil to do it now, { ask = …, detail = … } for a card,
    --  { refuse = for the model, note = for you }.
    function api.propose(tool, args)
        if not current then return { refuse = "Nothing is being built now: tell the user to draw where it goes first (build mode, Super+B).", note = "" } end
        if tool == "system_run" or tool == "system_read" then return judge_system(tool, args) end
        if tool == "creation_key" then
            local keys, order = tostring(args.keys or ""), tostring(args.order or "")
            if not keys:match("^[%w%+]+$") then return { refuse = "Keys like Super+u or Super+Shift+n.", note = "" } end
            if not (order == "open" or order:match("^emit [%w_]+") or order:match("^fact [%w_.]+ %S+$")) then
                return { refuse = "The order is «open» (an app), «emit EVENT» or «fact NAME VALUE», for the creation itself.", note = "" }
            end
            return { ask = string.format(tr("«%s» wants the key %s to: %s"), current.name, keys, order) .. "\n" .. tostring(args.why or ""), detail = tr("A key of its own") }
        end
        if tool ~= "creation_write" then return nil end
        local file = tostring(args.file or "")
        if not file:match("%.plm$") then return nil end
        local refused, ask, fine = judge(args.content)
        if #refused > 0 then
            return {
                refuse = "This version was refused: it asks to " .. table.concat(refused, "; ") .. ". A creation made in a chat may not do that. Do it without it (a pleamar service, or leave that part out) and tell the user plainly.",
                note = tr("Not allowed: ") .. table.concat(refused, "; ") .. ".",
            }
        end
        if #ask > 0 then
            local lines = {}
            for _, a in ipairs(ask) do lines[#lines + 1] = "· " .. a.words end
            args._ask = ask
            return { ask = string.format(tr("«%s» wants to:"), current.name) .. "\n" .. table.concat(lines, "\n") .. "\n" .. tostring(args.why or ""), detail = tr("New permissions") }
        end
        return nil
    end
    --  Doing it: `done(ok, words, extra)` answers the model.
    --  The agent's cursor, once it has tried something with it: its light on
    --  the monitor stays until told it is done (or a minute and a half).
    local poked = false
    function api.release_cursor()
        if not poked then return end
        poked = false
        run("pleamar-wm", { "agent", "done" }, function() end, { errors = true })
    end
    function api.perform(tool, args, done)
        if not current then done(false, "Nothing is being built.") return end
        local id = current.id
        if tool == "system_run" or tool == "system_read" then
            system_do(tool, args, done)
        elseif tool == "creation_poke" then
            --  What --say understands, and nothing else: it is the creation's
            --  own socket. press, drag, hold, wheel, type and key go through
            --  its hand as a person's would, and in pleamar-wm the agent's
            --  cursor glides there first: the user sees it being tried.
            local order = tostring(args.order or ""):gsub("[\r\n]", " ")
            local zone = "[%w_.#]+"
            local fine = order == "describe"
                or order:match("^press " .. zone .. "$") or order:match("^press " .. zone .. " [%w ]+$")
                or order:match("^hold " .. zone .. "$")
                or order:match("^drag " .. zone .. " %-?[%d.]+ %-?[%d.]+$")
                or order:match("^wheel " .. zone .. " %-?[%d.]+$")
                or order:match("^type " .. zone .. " .+$")
                or order:match("^key [%w+]+$")
                or order:match("^fact [%w_.]+ %S+$") or order:match("^emit [%w_]+") or order:match("^get [%w_.]+$")
            if not fine then
                done(false, "Not an order it understands: describe · press ZONE [right|middle] [2] · hold ZONE · drag ZONE DX DY · wheel ZONE N · type ZONE TEXT · key COMBO · fact NAME VALUE · emit EVENT · get NAME.")
                return
            end
            if order:match("^press ") or order:match("^hold ") or order:match("^drag ") or order:match("^wheel ") or order:match("^type ") or order:match("^key ") then
                poked = true
            end
            run("pleamar", { "--say", id, order }, function(out, code)
                done(code == 0, tostring(out or ""))
            end, { errors = true })
        elseif tool == "creation_read" then
            local t = read(id, tostring(args.file or ""))
            done(true, t or "(there is no such file yet)")
        elseif tool == "creation_check" then
            check(done)
        elseif tool == "creation_write" then
            local file, content = tostring(args.file or ""), tostring(args.content or "")
            if not file:match("^[a-z0-9_-]+%.[a-z]+$") then done(false, "Not a file of the creation.") return end
            if file:match("%.plm$") then
                local _, ask, fine = judge(content)
                approve(args._ask or ask, fine)
            end
            --  Another kind of surface, other monitors or another keyboard are
            --  only taken when the scene starts again: it does.
            local restart = file == "scene.plm" and shape_words(read(id, "scene.plm")) ~= shape_words(content)
            --  The version before, kept: «Undo» puts it back.
            local n = #current.versions + 1
            pcall(sys.call, "files.write", where(id) .. "/versions/" .. n .. "-" .. file, content)
            table.insert(current.versions, { n = n, file = file, why = tostring(args.why or ""), at = os.time() })
            pcall(sys.call, "files.write", where(id) .. "/" .. disk(id, file), content)
            if file == "scene.plm" then current.app = is_window(content) end
            record_save()
            if restart then stop_scene() end
            start_scene()
            if file:match("%.plm$") or file:match("%.luau$") then
                said = {}
                --  A moment for it to reload and its logic to run, so what that
                --  said is in the answer.
                after(1500, function()
                    check(function(ok, words)
                        --  Its first version on the screen: the sketch gives way.
                        if ok and file == "scene.plm" then fact.made = true end
                        done(true, (ok and "Written; it compiles, and it is on the screen now: " or "Written, but it does NOT compile (the screen keeps the last good version): ") .. words)
                    end)
                end)
            else
                done(true, "Written.")
            end
        elseif tool == "creation_look" and current.app then
            --  An app is a window wherever the desktop put it: found by its title.
            local title = tostring(read(id, "scene.plm") or ""):match('title:%s*"([^"]*)"') or ""
            local dest = STATE .. "/build-look.png"
            run("pleamar-wm", { "agent", "windows" }, function(out, code)
                local pid = nil
                for line in tostring(out or ""):gmatch("[^\n]+") do
                    if title ~= "" and line:find("«" .. title .. "»", 1, true) then pid = line:match("^%s*([%d%.]+)") end
                end
                if code ~= 0 or not pid then done(false, "Its window is not on the screen (or this desktop cannot show one): is it kind: window with a title?") return end
                run("pleamar-wm", { "agent", "look", pid, dest }, function(o2, c2)
                    done(c2 == 0, c2 == 0 and "The app's window as it is now." or tostring(o2), c2 == 0 and { image = { path = "/state/build-look.png" } } or nil)
                end, { errors = true })
            end, { errors = true })
        elseif tool == "creation_key" then
            current.keys = current.keys or {}
            table.insert(current.keys, tostring(args.keys))
            record_save()
            bind_key(current, tostring(args.keys), tostring(args.order), done)
        elseif tool == "creation_name" then
            local name = tostring(args.name or ""):gsub("[\r\n]", " "):sub(1, 40)
            if name == "" then done(false, "A name, please.") return end
            current.name = name
            text["build.name"] = name
            local ix = index()
            for _, item in ipairs(ix.list) do if item.id == id then item.name = name end end
            index_save(ix)
            record_save()
            done(true, "Called «" .. name .. "» now.")
        elseif tool == "creation_look" then
            local r = current.region
            local m = 24
            --  The surface as the scene says it is now, with where it was
            --  drawn, and a margin: what grew past the drawing is seen too.
            local x0, y0, x1, y1 = r.x, r.y, r.x + r.w, r.y + r.h
            local sx, sy, sw, sh = surface_box(read(id, "scene.plm"), r)
            if sx then x0, y0, x1, y1 = math.min(x0, sx), math.min(y0, sy), math.max(x1, sx + sw), math.max(y1, sy + sh) end
            x0, y0 = math.max(0, x0 - m), math.max(0, y0 - m)
            x1, y1 = math.min(r.mw, x1 + m), math.min(r.mh, y1 + m)
            local ox, oy = r.gx - r.x, r.gy - r.y
            local dest = STATE .. "/build-look.png"
            --  Where it is, in words: a picture alone does not say that its
            --  side is the monitor's edge, or 6 px short of it.
            local words = string.format("The creation on the screen now, with %d px around it. The picture is x %d…%d, y %d…%d of the monitor (%dx%d)", m, x0, x1, y0, y1, r.mw, r.mh)
            local edges = {}
            if x0 <= 0 then edges[#edges + 1] = "its left side is the monitor's left edge" end
            if x1 >= r.mw then edges[#edges + 1] = "its right side is the monitor's right edge" end
            if y0 <= 0 then edges[#edges + 1] = "its top is the monitor's top edge" end
            if y1 >= r.mh then edges[#edges + 1] = "its bottom is the monitor's bottom edge" end
            if #edges > 0 then words = words .. "; " .. table.concat(edges, ", ") end
            words = words .. "."
            if sx then
                words = words .. string.format(" Its surface is at %d,%d, %dx%d: %d px from the left edge, %d from the right, %d from the top, %d from the bottom.",
                    sx, sy, sw, sh, sx, r.mw - (sx + sw), sy, r.mh - (sy + sh))
            end
            --  Build mode's own marks (its edge, the corners) out of the
            --  picture, and the agent's light on the monitor's edge too.
            local lit = poked
            api.release_cursor()
            fact.looking = true
            after(lit and 450 or 120, function()
                run("grim", { "-g", string.format("%d,%d %dx%d", math.floor(ox + x0), math.floor(oy + y0), math.floor(x1 - x0), math.floor(y1 - y0)), dest }, function(out, code)
                    fact.looking = false
                    if code == 0 then
                        done(true, words, { image = { path = "/state/build-look.png" } })
                    else
                        done(false, "Could not take the picture: " .. tostring(out))
                    end
                end, { errors = true })
            end)
        elseif tool == "pleamar_docs" then
            local topic = tostring(args.topic or "reference")
            local argv = topic == "grammar" and { "--grammar" } or { "--docs", topic }
            run("pleamar", argv, function(out, code)
                if code ~= 0 then done(false, tostring(out)) return end
                local all = tostring(out)
                local find = tostring(args.find or ""):lower()
                if find == "" then
                    done(true, #all > 30000 and (all:sub(1, 30000) .. "\n[…cut: ask with `find` for the rest]") or all)
                    return
                end
                --  Only the paragraphs that mention it.
                local found, size = {}, 0
                for para in (all .. "\n\n"):gmatch("(.-)\n\n+") do
                    if para:lower():find(find, 1, true) then
                        if size + #para > 24000 then break end
                        found[#found + 1] = para
                        size += #para
                    end
                end
                done(true, #found > 0 and table.concat(found, "\n\n") or ("Nothing in «" .. topic .. "» mentions «" .. find .. "»."))
            end, { errors = true })
        else
            done(false, "Unknown tool.")
        end
    end

    -- ── pictures to show her what you mean ──
    --  Dropped on where you write (a file, or a picture from a browser), a
    --  piece of the screen, or what you copied: made a PNG no bigger than
    --  the model takes, in her engine's folder, and sent with your next
    --  message. Four at most.
    local refs, ref_n = {}, 0
    local function show_refs()
        local out = {}
        for k, r in ipairs(refs) do out[k] = { pic = r.local_path } end
        model.refs = out
    end
    local function hint(words)
        text["c.hint"] = words or ""
        if words then after(4000, function() text["c.hint"] = "" end) end
    end
    --  `what`: "sketch" (the user's drawing), "under" (what the screen had
    --  there), or nothing for a reference the user chose. The builder is
    --  told which is which: the pen's strokes and the wallpaper are not the
    --  design.
    take_picture = function(source, done_ok, what)
        if #refs >= 4 then hint(tr("Four pictures at most")) return end
        ref_n += 1
        local name = string.format("ref-%d-%d.png", os.time(), ref_n)
        local dest = STATE .. "/refs/" .. name
        run("mkdir", { "-p", STATE .. "/refs" }, function()
            run("magick", { source .. "[0]", "-resize", "1568x1568>", dest }, function(out, code)
                if code ~= 0 then hint(tr("That is not a picture it can read")) return end
                refs[#refs + 1] = { local_path = dest, path = "/state/refs/" .. name, what = what }
                show_refs()
                if done_ok then done_ok() end
            end, { errors = true })
        end)
    end
    --  A file's address as a path: file:///home/x/a%20b.png → /home/x/a b.png
    local function from_uri(u)
        local p = u:gsub("^file://[^/]*", "")
        return (p:gsub("%%(%x%x)", function(h) return string.char(tonumber(h, 16)) end))
    end
    local function dropped(data, mime)
        for line in tostring(data or ""):gmatch("[^\r\n]+") do
            local item = line:gsub("^%s+", ""):gsub("%s+$", "")
            local file = item:match("^file://") and from_uri(item) or (item:sub(1, 1) == "/" and item or nil)
            if file and file:match("%.build%.tar%.gz$") then
                if not current then import(file) end
            elseif not current then
                --  Pictures are for what is being built.
            elseif item:match("^file://") or item:sub(1, 1) == "/" then
                take_picture(item:match("^file://") and from_uri(item) or item)
            elseif item:match("^https?://") then
                --  A picture from a browser: fetched first, then made a PNG.
                local tmp = STATE .. "/refs/fetch-" .. os.time() .. ".bin"
                run("mkdir", { "-p", STATE .. "/refs" }, function()
                    run("curl", { "-sSL", "--max-time", "20", "--max-filesize", "20000000", "-o", tmp, item }, function(_, code)
                        if code == 0 then take_picture(tmp, function() run("rm", { "-f", tmp }) end) else hint(tr("That is not a picture it can read")) end
                    end)
                end)
            end
        end
    end
    on("drop:drop_area", dropped)
    on("drop:list_drop", dropped)
    --  A piece of the screen: build mode lets you drag over it (draw.luau),
    --  and hands the picture here.
    on("ref_shot", function()
        if current then fact.shooting = true end
    end)
    function api.add_picture(path)
        take_picture(path, function() run("rm", { "-f", path }) end)
    end
    --  A picture copied (a screenshot, an image from the browser): the
    --  paste button, or Ctrl+V in the panel —which, quiet, leaves text to
    --  the field it is typed in—.
    local function paste_picture(quiet)
        if not current then return end
        local tmp = STATE .. "/refs/paste-" .. os.time() .. ".png"
        run("mkdir", { "-p", STATE .. "/refs" }, function()
            run("sh", { "-c", 'wl-paste --list-types 2>/dev/null | grep -q "^image/" || exit 3; t=$(wl-paste --list-types | grep -m1 "^image/png" || wl-paste --list-types | grep -m1 "^image/"); wl-paste --type "$t" > "$0" 2>/dev/null && [ -s "$0" ]', tmp }, function(_, code)
                if code == 0 then take_picture(tmp, function() run("rm", { "-f", tmp }) end)
                elseif not quiet then hint(tr("Nothing copied is a picture")) end
            end)
        end)
    end
    on("ref_paste", function() paste_picture(false) end)
    on("key", function(name)
        if name == "Ctrl+v" and fact.building then paste_picture(true) end
    end)
    on("ref_remove", function(i)
        local k = (tonumber(i) or -1) + 1
        if refs[k] then table.remove(refs, k); show_refs() end
    end)
    --  For the message being sent: its pictures, and the chips emptied.
    function api.take_refs()
        if #refs == 0 then return nil, "" end
        local images, notes = {}, {}
        local SAYS = {
            sketch = "the user's rough drawing on the screen: where the piece goes and roughly its outline. The pen's mint strokes are only the pen, not a colour or a part of the design",
            under = "what the screen has there now, under the drawing (their desktop as it is, its wallpaper): context for where it sits, not part of the design",
        }
        for k, r in ipairs(refs) do
            images[k] = { path = r.path }
            notes[k] = string.format("%d — %s", k, SAYS[r.what] or "a picture the user chose: a reference for what they want. Copy the piece they mean, not the wallpaper, mockup frame or other windows around it")
        end
        local words = #refs == 1 and tr("a picture") or string.format(tr("%d pictures"), #refs)
        refs = {}
        show_refs()
        return images, words, "[The pictures with this message, in order: " .. table.concat(notes, ". ") .. ".]"
    end

    -- ── undo, keep, throw away ──
    on("b_undo", function()
        if not current or #current.versions == 0 then return end
        local last = table.remove(current.versions)
        local before = nil
        for k = #current.versions, 1, -1 do
            if current.versions[k].file == last.file then before = current.versions[k]; break end
        end
        local text_before = before and read(current.id, "versions/" .. before.n .. "-" .. last.file)
        if text_before then
            pcall(sys.call, "files.write", where(current.id) .. "/" .. disk(current.id, last.file), text_before)
        elseif last.file == "scene.plm" then
            local _, surface, w, h = shape(current.region)
            pcall(sys.call, "files.write", where(current.id) .. "/" .. disk(current.id, "scene.plm"), placeholder(current.name, surface, w, h))
        else
            pcall(sys.call, "files.remove", where(current.id) .. "/" .. disk(current.id, last.file))
        end
        record_save()
        if chat then chat.note(string.format(tr("Undone: %s"), last.why ~= "" and last.why or last.file)) end
    end)
    local function finish()
        api.release_cursor()
        refs = {}
        show_refs()
        if chat then
            local rows, history = chat.finish()
            current.rows, current.history = rows, history
        end
        record_save()
        current = nil
        fact.building = false
        fact["c.kept"] = false
        fact.made = false
        fact["reg.w"] = 0
        text["build.name"] = ""
        --  Told by hand: a logic's own change to a fact reaches no `fact:` handler.
        if api.finished then api.finished() end
    end
    -- ── what you keep runs with the session ──
    --  pleamar-wm reads two files of yours when the session starts: the
    --  creations to start (creations.autostart) and the surfaces they
    --  replaced, to keep hidden (creations.hide). Each line carries the
    --  creation's id, so it goes when the creation goes.
    local AUTOSTART = config_dir .. "/creations.autostart"
    local HIDE = config_dir .. "/creations.hide"
    --  The creation's lines out of a file, and then its new one if it has
    --  one, in a single step: two steps at once raced, and kept it twice.
    --  (grep -v with nothing left fails: the file is emptied all the same.)
    local function lines_set(file, id, line)
        run("sh", { "-c", [[mkdir -p "$(dirname "$0")" && touch "$0" && { grep -v -F "# creation $1" "$0" || true; } > "$0.new" && { [ -z "$2" ] || printf "%s\n" "$2" >> "$0.new"; } && mv "$0.new" "$0"]], file, id, line or "" })
    end
    local function forget_everywhere(id)
        lines_set(AUTOSTART, id)
        lines_set(HIDE, id)
        unbind_keys(id)
    end
    --  With the session from now on, or not: its line to start it and the
    --  one that keeps what it replaced hidden.
    local function with_session(record, yes)
        if not yes then
            lines_set(AUTOSTART, record.id)
            lines_set(HIDE, record.id)
            return
        end
        lines_set(AUTOSTART, record.id, string.format("%s --scene %s --no-hud  # creation %s", pleamar_path, path_of(record.id, "scene.plm"), record.id))
        local hide = nil
        if record.replaces then
            local prog, scene = tostring(record.replaces.who):match("^(%S+)%s*(.*)$")
            hide = string.format("%s\t%s\t%s  # creation %s", prog or "", scene or "", tostring(record.replaces.ns or ""), record.id)
        end
        lines_set(HIDE, record.id, hide)
    end
    on("b_keep", function()
        if not current then return end
        local record = current
        record.kept = true
        record.off = nil
        if record.app then
            --  An app: in the finder and the dock from now on; its window
            --  stays open, and it opens from there next time.
            install_app(record)
            log("build · kept the app " .. record.id)
            finish()
            return
        end
        with_session(record, true)
        if record.keys then
            for _, k in ipairs(record.keys) do log("build · its key " .. k .. " stays") end
        end
        --  It goes on running, on its own now: this scene may close.
        stop_scene(function()
            run("sh", { "-c", 'setsid -f "$0" --scene "$1" --no-hud >/dev/null 2>&1', pleamar_path, path_of(record.id, "scene.plm") })
        end)
        log("build · kept " .. record.id)
        finish()
    end)
    local discard
    on("b_discard", function() discard() end)
    discard = function()
        if not current then return end
        stop_scene()
        if current.app then run("rm", { "-f", desktop_file(current.id) }) end
        if current.replaces then set_hidden(current.replaces, false) end
        forget_everywhere(current.id)
        current.kept = false
        current.thrown = true
        local ix = index()
        for _, item in ipairs(ix.list) do if item.id == current.id then item.thrown = true end end
        index_save(ix)
        log("build · thrown away " .. current.id)
        finish()
    end
    --  Leaving it in the middle leaves it as it is: not kept, it stops (and
    --  what it replaced comes back); opening it again carries on.
    local function leave_current()
        if not current then return end
        stop_scene()
        if current.replaces and not current.kept then set_hidden(current.replaces, false) end
        --  One kept before and opened again: back to running on its own.
        --  A moment later and on its own: build mode may be closing, and its
        --  copy here must have gone first.
        if current.kept and not current.app and not current.off then
            run("sh", { "-c", [[setsid -f sh -c 'sleep 1; exec "$0" --scene "$1" --no-hud' "$0" "$1" >/dev/null 2>&1]], pleamar_path, path_of(current.id, "scene.plm") })
        end
        finish()
    end
    api.leave = leave_current
    on("finish", leave_current)

    -- ── one from someone else (a .build.tar.gz dropped on her chat) ──
    review = function(record)
        local content = read(record.id, "scene.plm") or ""
        local refused, ask, fine = judge(content)
        local function go()
            approve(ask, fine)
            record.pending = nil
            record_save()
            start_scene()
            if chat then chat.note(string.format(tr("«%s» is running. Ask me to change anything in it."), record.name)) end
        end
        if #refused > 0 then
            if chat then chat.note(tr("It does not run: it asks to ") .. table.concat(refused, "; ") .. ". " .. tr("Ask me to take that out.")) end
            return
        end
        if #ask == 0 then go() return end
        local lines = {}
        for _, a in ipairs(ask) do lines[#lines + 1] = "· " .. a.words end
        if chat then
            chat.card(string.format(tr("«%s» comes from someone else, and wants to:"), record.name) .. "\n" .. table.concat(lines, "\n"), tr("Before it runs"), function(yes)
                if yes then go() elseif chat then chat.note(tr("It stays as it is, not running. Ask me to change what it asks for.")) end
            end)
        end
    end
    import = function(path)
        monitors(function(list)
            local m = list[1]
            if not m then return end
            local id = "c" .. os.date("%Y%m%d%H%M%S")
            local tmp = STATE .. "/import/" .. id
            local dest = FOLDER .. "/" .. where(id)
            --  Unpacked apart; anything that is not a plain file (a link that
            --  could point at your things) refuses it.
            run("sh", { "-c", 'rm -rf "$0" && mkdir -p "$0" && tar xzf "$1" -C "$0" --no-same-owner --no-same-permissions && [ -f "$0/creation/scene.plm" ] && [ -z "$(find "$0" ! -type f ! -type d)" ] && mkdir -p "$2" && cp "$0/creation/scene.plm" "$2/$3.plm" && { [ -f "$0/creation/scene.luau" ] && cp "$0/creation/scene.luau" "$2/$3.luau"; [ -f "$0/creation/icon.svg" ] && cp "$0/creation/icon.svg" "$2/"; true; } && cat "$0/creation/manifest.json" 2>/dev/null; rc=$?; rm -rf "$0"; exit $rc',
                        tmp, path, dest, id }, function(out, code)
                if code ~= 0 then
                    if chat then chat.note(tr("That file is not a creation it can open.")) end
                    return
                end
                local ok, manifest = pcall(json.decode, tostring(out or ""))
                manifest = (ok and type(manifest) == "table") and manifest or {}
                local name = tostring(manifest.name or "Shared"):gsub("[%c]", ""):sub(1, 40)
                local w = math.max(40, math.min(m.w, tonumber(manifest.size and manifest.size.w) or 320))
                local h = math.max(30, math.min(m.h, tonumber(manifest.size and manifest.size.h) or 200))
                --  Its monitor is the one it was made on, someone else's: yours now.
                local scene = read(id, "scene.plm") or ""
                scene = scene:gsub('screens:%s*"[^"]*"', 'screens: "' .. m.name .. '"')
                pcall(sys.call, "files.write", where(id) .. "/" .. disk(id, "scene.plm"), scene)
                local r = { monitor = m.name, mw = m.w, mh = m.h, x = math.floor((m.w - w) / 2), y = math.floor((m.h - h) / 2), w = w, h = h, gx = m.x + math.floor((m.w - w) / 2), gy = m.y + math.floor((m.h - h) / 2), monitor_index = m.index }
                local record = { id = id, name = name, kind = tostring(manifest.kind or "widget"), app = manifest.app == true or is_window(scene), region = r, made = os.time(),
                                 approved = { run = {}, services = {} }, versions = {}, history = {}, rows = {}, kept = false, pending = true, from = "shared" }
                pcall(sys.call, "files.write", where(id) .. "/creation.json", record)
                local ix = index()
                table.insert(ix.list, { id = id, name = name, kind = record.kind, region = r })
                index_save(ix)
                log("build · imported " .. id .. " (" .. name .. ")")
                if current == nil and not fact.building then open(record) end
            end, { errors = true })
        end)
    end

    on("import", function()
        local path = tostring(text.import_path or "")
        if path:match("%.build%.tar%.gz$") and not current then import(path) end
    end)

    -- ── what you built: the list, open again, share, delete ──
    local shelf = {}          -- the list's rows, in order: creation ids
    local sure_id, sure_timer = nil, nil
    local KIND_WORDS = { bar = "A bar", dock = "A dock", widget = "A widget" }
    local function show_list()
        local out = {}
        shelf = {}
        for _, item in ipairs(index().list) do
            if not item.thrown then
                local record = load(item.id) or {}
                local what = record.app and tr("An app") or tr(KIND_WORDS[item.kind] or "A widget")
                local can = record.kept and not record.app
                what = what .. " · " .. (not record.kept and tr("not kept") or record.off and tr("off") or tr("kept"))
                out[#out + 1] = { name = tostring(record.name or item.name or item.id), what = what, kept = record.kept and 1 or 0, sure = item.id == sure_id and 1 or 0,
                                  can = can and 1 or 0, on = (can and not record.off) and 1 or 0 }
                shelf[#shelf + 1] = item.id
            end
        end
        model.list = out
    end
    on("creations", function()
        show_list()
        fact.view = "list"
    end)
    on("fact:locale", show_list)
    api.show_list = show_list
    on("l_open", function(i)
        local id = shelf[(tonumber(i) or -1) + 1]
        local record = id and load(id)
        if not record or current then return end
        stop_kept(id)
        open(record)
    end)
    --  Off without deleting it: it stops, does not start with the session,
    --  and what it replaced comes back; on, all of it again.
    on("l_toggle", function(i)
        local id = shelf[(tonumber(i) or -1) + 1]
        local record = id and load(id)
        if not record or not record.kept or record.app or (current and current.id == id) then return end
        record.off = not record.off or nil
        if record.off then
            stop_kept(id)
            with_session(record, false)
            if record.replaces then set_hidden(record.replaces, false) end
            log("build · off " .. id)
        else
            with_session(record, true)
            if record.replaces then set_hidden(record.replaces, true) end
            run("sh", { "-c", 'setsid -f "$0" --scene "$1" --no-hud >/dev/null 2>&1', pleamar_path, path_of(id, "scene.plm") })
            log("build · on " .. id)
        end
        pcall(sys.call, "files.write", where(id) .. "/creation.json", record)
        show_list()
    end)
    on("l_remove", function(i)
        local id = shelf[(tonumber(i) or -1) + 1]
        if not id then return end
        --  Pressed once, it asks to be sure; again within four seconds, it goes.
        if sure_id ~= id then
            sure_id = id
            if sure_timer then cancel(sure_timer) end
            sure_timer = after(4000, function() sure_timer = nil; sure_id = nil; show_list() end)
            show_list()
            return
        end
        if sure_timer then cancel(sure_timer); sure_timer = nil end
        sure_id = nil
        --  The one open in the chat: thrown away from there, or leaving it
        --  would start it again and save it as kept.
        if current and current.id == id then
            discard()
            show_list()
            return
        end
        local record = load(id)
        stop_kept(id)
        if record then
            if record.app then run("rm", { "-f", desktop_file(id) }) end
            if record.replaces then set_hidden(record.replaces, false) end
            forget_everywhere(id)
            record.kept = false
            record.thrown = true
            pcall(sys.call, "files.write", where(id) .. "/creation.json", record)
        end
        --  Its folder stays, out of the list: nothing of yours is lost by a
        --  slip of the finger.
        local ix = index()
        for _, item in ipairs(ix.list) do if item.id == id then item.thrown = true end end
        index_save(ix)
        log("build · deleted " .. id)
        show_list()
    end)
    --  Shared: its scene, its logic and its icon, with what it is and how big,
    --  in a file to give anyone (#share-you-creation). Never its
    --  conversation, which is yours.
    on("l_share", function(i)
        local id = shelf[(tonumber(i) or -1) + 1]
        local record = id and load(id)
        if not record then return end
        local name = tostring(record.name):gsub("[^%w%-_ ]", ""):gsub("%s+", "-")
        if name == "" then name = id end
        local manifest = { name = record.name, kind = record.kind, app = record.app == true,
                           size = { w = record.region and record.region.w or 0, h = record.region and record.region.h or 0 },
                           made_with = "pleamar-wm · Build" }
        local stage = STATE .. "/share/" .. id
        run("xdg-user-dir", { "DOWNLOAD" }, function(dir)
            dir = tostring(dir or ""):gsub("%s+$", "")
            if dir == "" then dir = home_dir end
            local dest = dir .. "/" .. name .. ".build.tar.gz"
            run("sh", { "-c", 'rm -rf "$0" && mkdir -p "$0/creation" && cp "$1/$2.plm" "$0/creation/scene.plm" && { [ -f "$1/$2.luau" ] && cp "$1/$2.luau" "$0/creation/scene.luau"; [ -f "$1/icon.svg" ] && cp "$1/icon.svg" "$0/creation/"; true; } && cat > "$0/creation/manifest.json" && tar czf "$3" -C "$0" creation && rm -rf "$0"',
                        stage, FOLDER .. "/" .. where(id), id, dest }, function(out, code)
                fact["l.on"] = true
                text["l.shared"] = code == 0 and string.format(tr("Shared as %s: give it to anyone, or post it in #share-you-creation."), dest) or (tr("Could not share it: ") .. tostring(out))
                after(12000, function() text["l.shared"] = ""; fact["l.on"] = false end)
            end, { errors = true, input = json.encode(manifest) })
        end)
    end)

    --  The chat lends what it needs to hold the builder's conversation:
    --  begin(history, rows), finish() → rows, history, note(text), show().
    --  The engine keeps the conversation of the one being built here too.
    function api.save_talk(rows, history)
        if not current then return end
        current.rows, current.history = rows, history
        record_save()
    end
    shared.creations = api
    return api
end
