# Development guide

## Requirements

- Windows 10 or 11
- Node.js 20.19 or newer
- current stable Rust toolchain
- Tauri v2 prerequisites for Windows

## Install and run

```powershell
npm ci
npm run tauri dev
```

The development runtime launches the Vite frontend and the Tauri app together.
The installed app and a development build both use local network ports, so close
the installed app before testing two live instances on the same machine.

## Frontend preview without two PCs

Run the Vite server:

```powershell
npm run dev
```

Then open one of these URLs in a browser:

```text
http://localhost:1420/dev/preview.html?state=firstrun#/home
http://localhost:1420/dev/preview.html?state=receiving#/home
http://localhost:1420/dev/preview.html?state=disconnected#/audio
http://localhost:1420/dev/preview.html?state=sending#/input
```

Available preview states are `firstrun`, `disconnected`, `public`,
`reconnecting`, `receiving`, `sending`, and `idle`. The preview uses
`dev/mock-tauri.ts`, so it renders the real interface without touching Windows
audio, networking, or input hooks.

## Checks

```powershell
# Frontend type-check and production bundle
npm run build

# Rust formatting, lints, and tests
cd src-tauri
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The release smoke test requires two real Windows PCs, audio devices, and a human
ear. Follow [QA-CHECKLIST.md](QA-CHECKLIST.md) before shipping an installer.

## Build an installer

```powershell
npm run tauri build
```

Tauri writes the Windows bundles below `src-tauri/target/release/bundle/`.
Never commit generated installers, `target/`, `dist/`, logs, or local `.env`
files.
