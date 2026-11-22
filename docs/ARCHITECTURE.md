# Trovul architecture

Trovul is a Windows desktop application with a React interface and a Rust
runtime. Its job is deliberately narrow: make two PCs on the same trusted local
network behave like one desk.

## System at a glance

```text
React + TypeScript UI
        │ Tauri commands and events
        ▼
Rust application runtime
 ├─ discovery and pairing
 ├─ control link and reconnects
 ├─ audio capture, transport, jitter, and playback
 ├─ Windows audio-session mixer
 ├─ keyboard, mouse, clipboard, and file sharing
 ├─ tray, notifications, hotkeys, and settings
 └─ optional Stream Deck / Companion controllers
```

## Main pieces

| Area | Location | Responsibility |
|---|---|---|
| Desktop interface | `src/` | React screens, typed Tauri bridge, UI primitives, and visual state. |
| Native runtime | `src-tauri/src/` | Networking, pairing, Windows integration, and application lifecycle. |
| Audio engine | `src-tauri/src/audio/` | WASAPI capture, PCM packets, jitter handling, DSP, and playback. |
| Mixer | `src-tauri/src/audio_sessions.rs` | Windows audio-session discovery, meters, volume, mute, and routing. |
| Shared input | `src-tauri/src/input/` | Monitor layout, hooks, crossing behavior, and replay on the paired PC. |
| Controllers | `streamdeck/` | Stream Deck plug-in backed by the opt-in Companion API. |

## Connection model

1. Each app discovers peers on the local network.
2. A user accepts the first connection request; the PCs retain their pairing.
3. A persistent control link carries state changes, mixer operations, input,
   clipboard negotiation, and reconnection signals.
4. Audio travels on a dedicated low-latency path with an adaptive jitter buffer.

The application deliberately supports one active paired PC at a time. That keeps
the interaction model predictable: one desk, two machines, no routing maze.

## Trust boundary

Trovul is intended for a trusted local network, not internet exposure. Pair only
PCs you control, keep the Companion API disabled unless you use it, and report
security concerns through [SECURITY.md](../SECURITY.md).

## Design principles

- **Fast paths stay native.** Audio, input, and Windows integration live in Rust.
- **The interface stays simple.** React renders the state; it does not own the
  audio or networking engine.
- **A bad network should recover.** Reconnect, device changes, sleep, and Wi-Fi
  drops are expected states, not exceptional ones.
- **The desk comes first.** Features should reduce switching and setup, not add
  another control surface to babysit.
