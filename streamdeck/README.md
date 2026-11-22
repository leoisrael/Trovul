# Trovul for Stream Deck

A Stream Deck plugin (SDK 3, Stream Deck 7.1+, Node 24) that controls Trovul through its companion API.

| Action | Where | What it does |
|---|---|---|
| **Scene** | key | Applies a saved Trovul scene on both PCs. The key shows the scene's name. |
| **Where the sound goes** | key | Hear → Send → Off, or one fixed direction (press again to turn it off). Lit while active. |
| **Mute** | key | Mutes the master, the other PC's sound here, or an app by name, on this PC or the other one. Red while muted. |
| **Volume** | dial (Stream Deck +) or key | Turn for volume (2 % per tick), press to mute. The touch strip shows the level. |
| **Follow the pointer** | key | Turns Trovul's "follow the pointer" on or off. |

## How it connects

Trovul → Settings → **Stream Deck & controllers** must be on. While it is, Trovul writes `companion.json` (port and token) into `%APPDATA%\com.trovul.app`. The plugin reads that file, so there is no token to type. It reconnects every 3 s when Trovul isn't running. A key pressed while Trovul is unreachable shows the Stream Deck alert.

Apps are matched by the name Trovul's mixer shows ("Discord", "chrome"…), because process ids change every time an app starts.

## Build, test, pack

```powershell
npm install
npm run build        # esbuild → com.trovul.streamdeck.sdPlugin/bin/plugin.js
npm test             # needs Node 24 (like Stream Deck): see below
npx @elgato/cli validate com.trovul.streamdeck.sdPlugin
npx @elgato/cli pack com.trovul.streamdeck.sdPlugin --output dist
```

`test/harness.mjs` runs the built plugin the way Stream Deck does, with no Stream Deck:
- it plays both the Stream Deck app (the WebSocket the SDK registers with) and Trovul's companion API;
- it presses keys, turns a dial and drops the connection;
- it checks what the plugin says to each side (19 checks).

`scripts/icons.mjs` writes the SVG icons. The plugin icon must be PNG (`imgs/plugin.png`, `@2x`) and is committed.

The `.streamDeckPlugin` file from `pack` installs with a double-click.
