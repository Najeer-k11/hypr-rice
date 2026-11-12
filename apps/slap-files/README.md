# Slap File Manager

An Explorer-style file manager for Hyprland. Rust + GTK4 (no libadwaita), themed live from
the matugen palette of your wallpaper — same look and engine as rice-settings. Built to stay light
on RAM (~40 MB PSS): CPU renderer, Nerd Font glyphs instead of an icon theme, lazily built rows.

    slap-files [path]     # open a folder, or a file's folder with the file selected
                          # (adds a tab if a window is already open)

## Install

    ./install.sh            # ~/.local
    ./install.sh --default  # + default app for folders and for "show in folder" (browsers etc.)
    cd packaging/arch && makepkg -si

## Keys

| key | action |
|---|---|
| alt+← / alt+→ / mouse back-forward | history |
| alt+↑ / backspace | up (lands on the folder you came from) |
| alt+home | home |
| ctrl+l / f4 | type a path (`~`, relative and `file://` work) |
| ctrl+f / f3, or just start typing | filter this folder |
| ctrl+h | hidden files |
| ctrl+1 / ctrl+2 | details / large icons |
| f5 / ctrl+r | refresh |
| enter / double-click | open |
| esc | clear filter, then selection |
| ctrl+c / ctrl+x / ctrl+v | copy / cut / paste (works with other file managers) |
| ctrl+shift+c | copy path |
| f2 | rename |
| delete / shift+delete | move to trash / delete permanently |
| ctrl+shift+n | new folder |
| ctrl+z | undo (rename, move, copy, trash, new) |
| ctrl+a | select all |
| menu / shift+f10 / right-click | context menu |
| ↓ in search | jump into the results |
| ctrl+t / ctrl+w | new tab / close tab |
| ctrl+shift+t | reopen closed tab |
| ctrl+tab, ctrl+pgdn / ctrl+shift+tab, ctrl+pgup | next / previous tab |
| alt+1…8 / alt+9 | go to tab n / last tab |
| middle-click folder or place | open in new tab (middle-click a tab closes it) |
| alt+p | preview pane |
| alt+enter | properties |
| ctrl+w / ctrl+q | close |

## Roadmap

1. ✅ browsing: theme, sidebar, address bar, details + grid views, filter, status bar, live updates
2. ✅ file operations: copy/cut/paste + progress/cancel, rename, trash/delete, new folder/file, context menu, drag & drop, undo
3. ✅ tabs, recursive search, properties, preview pane, pin to quick access, open with, terminal here
4. ✅ this pc: drives + free space, mount/unlock, safe eject, disk images, trash view, compress/extract
5. ✅ thumbnails, "show in folder" (FileManager1), packaging (.desktop, install.sh, PKGBUILD)

Drag & drop: plain drag moves within a drive and copies across drives; hold ctrl to copy,
shift to move. Drop onto folders, empty space, or breadcrumb segments.

Copies use the kernel's `copy_file_range` (instant reflinks on btrfs, no userspace buffer).
Name clashes ask once per batch: skip / keep both / replace (folders merge).

Search filters the current folder as you type; after a short pause (or on enter) it also
searches subfolders (same drive, no symlinked folders, first 5000 hits) with a folder column.

Terminal: `$TERMINAL`, else the first of kitty, foot, alacritty, wezterm, ghostty, konsole, …

## Drives & safe eject

Drives come from udisks2 over D-Bus (no gvfs needed). Click a drive to mount it (encrypted
drives ask for the passphrase); `.iso`/`.img` files can be mounted from their menu.

Eject (button next to the drive, or on its card in *this pc*) never pulls a drive out from
under data:

1. refuses while a copy/move/archive job in the app still touches the drive
2. moves every tab off the drive
3. flushes the drive's cached writes (`syncfs`) and shows how much is still waiting
4. unmounts every filesystem on it — never forced; if something holds it, names the processes
5. locks unlocked encrypted containers, then powers the drive off (images: detaches them)

Only then does it say *safe to remove*. Closing the window while a job runs asks first; the
process stays alive until the job has finished or cleaned up.

## Thumbnails

Large-icon view shows thumbnails from the shared freedesktop cache (`~/.cache/thumbnails`,
reused by and from other apps). New ones: images in-process, videos via ffmpeg, PDFs via
pdftoppm, other types via installed thumbnailers (glycin…). Two background threads, newest
first, at most ~96 thumbnails (~6 MB) kept in memory.

## Trash & archives

The trash view (sidebar) shows the home trash plus `.Trash-$uid` on mounted drives, with the
original location and deletion date: restore, delete for good, empty. Dropping files on the
trash trashes them.

*compress to* makes zip / tar.zst / tar.gz / tar.xz; *extract here* handles anything bsdtar
reads (zip, tar.*, 7z, rar, iso…) and doesn't nest a lone top-level folder.

Preferences and quick-access pins: `~/.config/slap/files.json`.
