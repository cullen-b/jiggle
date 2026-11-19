<p align="center"><img src="docs/logo.svg" width="160" alt="jiggle logo"></p>

# jiggle

Try it: https://cullen-b.github.io/jiggle/

Like it? [Buy me a coffee](https://payme.cullenbaker.com).

Shake the mouse anywhere and your Dock apps appear in a ring around the pointer. The ring is a pie:
click anywhere in an app's slice to open (or switch to) it. As you move out toward an app, its slice widens and
it grows while the far side makes room, so the app you're heading for gets easier to hit. Drag an app around the
ring to reorder it; the order is yours and never changes unless you drag (new apps join at the end). Right-click an app for its Dock menu (Keep in /
Remove from Dock, Show in Finder, Hide, Quit, and Force Quit with Option). Click outside the ring or press
Esc to close it.

Right-click the ring's center hub (or click the menu bar icon) for the Jiggle menu: show all Dock apps or
only open ones, ring opacity, add apps, Settings…, and Quit. Apps you add stay in the ring even when they're
closed and not in the Dock; right-click an app and choose Keep in Jiggle or Remove from Jiggle to do the same.
Choose Never Show in Jiggle to hide an app from the ring for good (Settings… lists hidden apps to show again).

- ~330 KB binary, AppKit only, no runtime besides the system frameworks
- Idle cost: one event-tap callback per mouse move; nothing polls

## Install

```sh
git clone https://github.com/cullen-b/jiggle.git && cd jiggle && ./install.sh
```

Installs the latest signed release to `~/Applications/Jiggle.app` and starts it now and at every login
(`~/Library/LaunchAgents/local.jiggle.plist`). Grant it **Accessibility** when asked.
`./install.sh --source` builds from this checkout instead.

Uninstall: `launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/local.jiggle.plist && rm -rf ~/Applications/Jiggle.app ~/Library/LaunchAgents/local.jiggle.plist`

## Updates

Jiggle checks GitHub 20 seconds after it starts and every 6 hours after that. When a newer release exists it
downloads it, installs it only if it is signed by the jiggle certificate pinned in
`scripts/requirement.txt`, and restarts. Because every release carries the same signature, macOS keeps the
Accessibility permission across updates. Set `JIGGLE_NO_UPDATE=1` to turn this off.

Releasing: bump `version` in `Cargo.toml`, then push a matching tag (`git tag v0.3.0 && git push origin v0.3.0`).
The `release` workflow builds a universal binary, signs it and publishes `Jiggle.zip`.

## Settings

Change them in **Settings…** (Jiggle menu), or from a terminal: they live in the macOS defaults domain
`local.jiggle`, so you or an AI agent can read and write them with `defaults`. Jiggle picks up changes the next
time the ring opens; no restart needed.

| Key | Type | Default | What it does |
|---|---|---|---|
| `OpenAppsOnly` | bool | `false` | Show only running apps (plus apps you added) instead of all Dock apps |
| `RingOpacity` | int, 10–100 | `80` | Opacity of the ring's background, in percent |
| `ExtraApps` | array of `.app` paths | empty | Apps always shown in the ring, even when closed and not in the Dock |
| `HiddenApps` | array of `.app` paths | empty | Apps never shown in the ring, even when open or in the Dock |
| `ShakeSensitivity` | int, 0–5 | `3` | How easily a mouse shake opens the ring: `1` very firm … `5` very light; `0` turns shaking off (use the shortcut) |
| `AppOrder` | array of `.app` paths | filled in as apps first appear | The ring's order, clockwise from 12 o'clock. Apps not listed go at the end; dragging in the ring rewrites it |
| `Shortcut` | string | unset (off) | System-wide shortcut that opens and closes the ring, e.g. `ctrl+opt+space`. Modifiers: `cmd`, `ctrl`, `opt`, `shift`; key: a letter, digit, `space`, `return`, `tab`, `f1`–`f20`, an arrow (`left`…), or `minus`, `equal`, `comma`, `period`, `slash`, `semicolon`, `quote`, `backslash`, `grave`, `leftbracket`, `rightbracket`, `delete`. Needs `cmd`, `ctrl` or `opt` unless it's an F-key |

```sh
defaults read local.jiggle                                              # current settings
defaults write local.jiggle OpenAppsOnly -bool true
defaults write local.jiggle RingOpacity -int 60
defaults write local.jiggle ExtraApps -array-add "/System/Applications/Notes.app"
defaults write local.jiggle ExtraApps -array "/Applications/Safari.app" "/System/Applications/Notes.app"
defaults write local.jiggle HiddenApps -array-add "/System/Applications/Messages.app"
defaults write local.jiggle ShakeSensitivity -int 4                     # a lighter shake opens it
defaults write local.jiggle Shortcut -string "ctrl+opt+space"           # turn the shortcut on
defaults delete local.jiggle Shortcut                                   # turn it off again
defaults delete local.jiggle RingOpacity                                # back to the default
```

## Tuning

Compile-time constants at the top of `src/main.rs`: `ICON`, `DISMISS_DISTANCE`, and the aiming feel
(`AIM_GROW`, `AIM_ICON_GROW`). Shake sensitivity is a setting (above); `shake_params` maps its levels.
