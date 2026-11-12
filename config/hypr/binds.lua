-- binds.lua — SUPER is the main modifier.  Cheatsheet: SUPER + /
local M = "SUPER"
local function k(keys) return M .. " + " .. keys end
local exec = hl.dsp.exec_cmd

------------------------------------------------------------------ APPS ----
hl.bind(k("Return"),          exec("kitty"))
hl.bind(k("SHIFT + Return"),  exec("kitty --class kitty-float"))
hl.bind(k("Space"),           exec("pkill fuzzel || fuzzel"))
hl.bind(k("R"),               exec("pkill fuzzel || fuzzel"))
hl.bind(k("E"),               exec("slap-files"))
hl.bind(k("B"),               exec("zen-browser"))
hl.bind(k("V"),               exec("rice-clip"))
hl.bind(k("W"),               exec("rice-wallpicker")                      )
hl.bind(k("ALT + W"),         exec("wall pick"))                       -- old fuzzel list
hl.bind(k("SHIFT + W"),       exec("wall random"))
hl.bind(k("I"),               exec("rice-settings"))                   -- settings app
hl.bind(k("slash"),           exec("rice-keys"))
hl.bind(k("ALT + P"),         exec("rice-toy pipes"))                  -- pipes
hl.bind(k("CTRL + B"),        exec("pkill -SIGUSR1 waybar"))           -- toggle bar

---------------------------------------------------------------- SESSION ----
hl.bind(k("L"),               exec("loginctl lock-session"))
hl.bind(k("Escape"),          exec("loginctl lock-session"))
hl.bind(k("SHIFT + Escape"),  exec("rice-power"))
hl.bind(k("N"),               exec("makoctl mode -t do-not-disturb && notify-send -t 1200 'Do not disturb' \"$(makoctl mode | grep -q do-not-disturb && echo on || echo off)\""))
hl.bind(k("SHIFT + N"),       exec("makoctl dismiss --all"))

----------------------------------------------------------- SCREENSHOTS ----
hl.bind("Print",              exec("rice-shot region"))
hl.bind("SHIFT + Print",      exec("rice-shot screen"))
hl.bind(k("SHIFT + S"),       exec("rice-shot region"))

---------------------------------------------------------------- WINDOWS ----
hl.bind(k("Q"),               hl.dsp.window.close())
hl.bind(k("F"),               hl.dsp.window.fullscreen({ mode = "fullscreen" }))
hl.bind(k("M"),               hl.dsp.window.fullscreen({ mode = "maximized" }))
hl.bind(k("T"),               hl.dsp.window.float({ action = "toggle" }))
hl.bind(k("C"),               hl.dsp.window.center())
hl.bind(k("P"),               hl.dsp.window.pin())
hl.bind(k("J"),               hl.dsp.layout("togglesplit"))
hl.bind(k("G"),               hl.dsp.group.toggle())
hl.bind(k("Tab"),             exec("rice-overview"))           -- workspace overview
hl.bind("ALT + Tab",          exec("pkill fuzzel || rice-windows all"))   -- switcher: all windows
hl.bind("ALT + SHIFT + Tab",  exec("pkill fuzzel || rice-windows ws"))    -- switcher: this workspace

-- focus / move / resize: arrows and vim keys both work
local dirs = { left = "h", right = "l", up = "k", down = "j" }
local step = 40
local delta = { left = {-step, 0}, right = {step, 0}, up = {0, -step}, down = {0, step} }
for dir, vim in pairs(dirs) do
    for _, key in ipairs({ dir, vim }) do
        if key ~= "j" and key ~= "l" then -- SUPER+J = togglesplit, SUPER+L = lock
            hl.bind(k(key), hl.dsp.focus({ direction = dir }))
        end
        hl.bind(k("SHIFT + " .. key), hl.dsp.window.move({ direction = dir }))
        hl.bind(k("CTRL + "  .. key),
            hl.dsp.window.resize({ x = delta[dir][1], y = delta[dir][2], relative = true }),
            { repeating = true })
    end
end

hl.bind(k("mouse:272"), hl.dsp.window.drag(),   { mouse = true })
hl.bind(k("mouse:273"), hl.dsp.window.resize(), { mouse = true })

------------------------------------------------------------- WORKSPACES ----
for i = 1, 10 do
    local key = i % 10
    hl.bind(k(key),              hl.dsp.focus({ workspace = i }))
    hl.bind(k("SHIFT + " .. key), hl.dsp.window.move({ workspace = i }))
end
hl.bind(k("grave"),          hl.dsp.focus({ workspace = "previous" }))
hl.bind(k("mouse_down"),     hl.dsp.focus({ workspace = "e+1" }))
hl.bind(k("mouse_up"),       hl.dsp.focus({ workspace = "e-1" }))
hl.bind(k("bracketright"),   hl.dsp.focus({ workspace = "e+1" }))
hl.bind(k("bracketleft"),    hl.dsp.focus({ workspace = "e-1" }))

-- scratchpad
hl.bind(k("S"),              hl.dsp.workspace.toggle_special("scratch"))
hl.bind(k("ALT + S"),        hl.dsp.window.move({ workspace = "special:scratch" }))

------------------------------------------------------------ MEDIA KEYS ----
local L = { locked = true, repeating = true }
hl.bind("XF86AudioRaiseVolume",  exec("rice-osd vol up"),   L)
hl.bind("XF86AudioLowerVolume",  exec("rice-osd vol down"), L)
hl.bind("XF86AudioMute",         exec("rice-osd vol mute"), { locked = true })
hl.bind("XF86AudioMicMute",      exec("wpctl set-mute @DEFAULT_AUDIO_SOURCE@ toggle"), { locked = true })
hl.bind("XF86MonBrightnessUp",   exec("rice-osd bri up"),   L)
hl.bind("XF86MonBrightnessDown", exec("rice-osd bri down"), L)
hl.bind("XF86AudioNext",  exec("playerctl next"),       { locked = true })
hl.bind("XF86AudioPrev",  exec("playerctl previous"),   { locked = true })
hl.bind("XF86AudioPlay",  exec("playerctl play-pause"), { locked = true })
hl.bind("XF86AudioPause", exec("playerctl play-pause"), { locked = true })

------------------------------------------------------ CLICK-OUTSIDE ----
-- close any open fuzzel panel when clicking outside it (click still passes through)
hl.bind("mouse:272", exec("rice-clickout"), { non_consuming = true })
hl.bind("mouse:273", exec("rice-clickout"), { non_consuming = true })
