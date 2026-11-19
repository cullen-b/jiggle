<p align="center"><img src="docs/logo.svg" width="160" alt="jiggle logo"></p>

# jiggle

Try it: https://cullen-b.github.io/jiggle/

Like it? [Buy me a coffee](https://payme.cullenbaker.com).

Shake the mouse anywhere and your Dock apps appear in a ring around the pointer. The ring is a pie:
click anywhere in an app's slice to open (or switch to) it. Right-click an app for its Dock menu (Keep in /
Remove from Dock, Show in Finder, Hide, Quit, and Force Quit with Option). Click outside the ring or press
Esc to close it.

The menu bar icon chooses between all Dock apps and only open apps, sets the ring's opacity, and quits.

Order matches the Dock: Finder, pinned apps, then other running apps (dot = running).

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

## Tuning

Constants at the top of `src/main.rs`: `SHAKE_REVERSALS`, `SHAKE_MIN_LEG`, `SHAKE_WINDOW` (shake
sensitivity), `ICON`, `DISMISS_DISTANCE`.
