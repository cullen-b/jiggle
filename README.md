<p align="center"><img src="docs/logo.svg" width="160" alt="jiggle logo"></p>

# jiggle

Try it: https://cullen-b.github.io/jiggle/

Shake the mouse anywhere and your Dock apps appear in a ring around the pointer. Point toward one
and click to open (or switch to) it. Esc, right-click, any key, or clicking the middle dismisses it.

Order matches the Dock: Finder, pinned apps, then other running apps (dot = running).

- ~330 KB binary, AppKit only, no runtime besides the system frameworks
- Idle cost: one event-tap callback per mouse move; nothing polls

## Install

```sh
chmod +x install.sh && ./install.sh
```

Builds `~/Applications/Jiggle.app`, starts it now and at every login (`~/Library/LaunchAgents/local.jiggle.plist`).
Grant it **Accessibility** when asked. The app is ad-hoc signed, so after a rebuild remove and re-add it in
the Accessibility list.

Uninstall: `launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/local.jiggle.plist && rm -rf ~/Applications/Jiggle.app ~/Library/LaunchAgents/local.jiggle.plist`

## Tuning

Constants at the top of `src/main.rs`: `SHAKE_REVERSALS`, `SHAKE_MIN_LEG`, `SHAKE_WINDOW` (shake
sensitivity), `ICON`, `DISMISS_DISTANCE`.
