# Changelog

## 0.7.10 — unreleased

### Fixes
- **Pop-ups left an empty strip** in the corner after the last card went. The window was shown with a Win32 call that Tauri's window layer never saw, so its hide did nothing. Showing and hiding now both go through Win32. Cards the backend dismisses also slide out instead of vanishing.

## 0.7.9

### Product
- **Stream Deck plugin** (`streamdeck/`, Stream Deck 7.1+).
  - **Keys.** Scenes, where the sound goes, mute (the master, the other PC's sound, or an app by name, on either PC) and follow the pointer.
  - **Dials.** On a Stream Deck + they turn the volume and press to mute, and the touch strip shows the level.
  - **Setup.** It connects by itself while Settings → Stream Deck & controllers is on, with no token to type.
  - **Tested** end to end against a simulated Stream Deck and Trovul (19 checks), and validated with Elgato's CLI.
- **Scenes are applied by the backend**, so a controller can switch them with the window closed.

### Companion API
- New messages: `get_state`, `apply_scene`, `set_direction`, `set_follow_pointer` and `adjust` (volume or mute of the master, the incoming sound or an app by name, on either PC; answered with `target`).
- A `state` message is pushed whenever the direction, settings, scenes or link change.
- While it runs, Trovul writes `companion.json` (port, token) to the app-data folder, for controllers on this PC.

## 0.7.8

### Product
- **Pop-ups.** Trovul's own cards in the corner of the screen, above the clock, in the style of the tray mixer. They say when:
  - a PC links or the link drops (unless you disconnected);
  - an image arrives on the clipboard;
  - the other PC switches the sound;
  - an admin window can't be reached;
  - an update is on the other PC ("Update now").

  They never take the focus, stay away from games in exclusive full screen and presentations, and fade after a few seconds unless they have a button or progress. They replace the Windows notifications (Settings → Pop-ups).
- **Send only the apps you choose** (Windows 11 on the sending PC). While a PC sends its sound, its Send strip has **All | Apps**.
  - **Choosing.** With **Apps**, every app strip gets a "➜ other PC" switch, lit when the app goes there. Under it is a bar for how loud it is on the other PC, separate from its dial (what plays on its own PC). That gives you a stream mix apart from your headset mix.
  - **At a glance.** A line on top lists what goes there, apps not playing shown dimmed.
  - **Apps come and go.** Chosen apps that start or stop playing are picked up within a second, and choices are kept by app name.
  - **From either PC.** Both PCs can set it, since it works through the link.
  - **Older Windows.** On Windows 10, "All sound" stays the only option.
  - **Protocol 2.5.** Adds `SetSendMode` and `SetAppSend`.
- **Follow the pointer.** In the headset, the PC you're using is louder: with the pointer on this PC, the other PC's sound drops (to 30 % by default, Settings → Audio); with it on the other PC, this PC's apps drop instead.
  - **Where.** Turn it on with the "Follow pointer" button in the Mixer (shown while this PC plays the other's sound). The decks say which PC is "In focus" and which is "Quieter".
  - **Your volumes stay put.** This PC's apps are lowered with a separate multiplier, so the dials never move. If Trovul stops while they're lowered, it puts them back at the next start.
  - **Needs keyboard & mouse sharing** on both PCs.
- **Files through the clipboard.** Copy files or folders in one PC's Explorer, paste them in the other's.
  - **Small copies.** Up to 50 MB they arrive by themselves, and pasting just works.
  - **Big copies.** Above that, a pop-up asks first ("israel-sil copied 3 files · 2.1 GB — Receive"), so a big copy you made for this PC alone never crosses the network by itself. It then saves to Downloads\Trovul with a progress bar and Cancel, and puts the files on the clipboard.
  - **Safety.** The sending PC only serves the files it listed, and the receiving PC only writes inside its target folder (paths with "..", drives or roots are refused).
  - **Protocol 2.4.** Adds `ClipOffer.files` and `GetClipboardFileChunk`.

## 0.7.6

### Keyboard & Mouse
- **Admin windows, UAC prompts and the lock screen.** New option on the Keyboard & Mouse page, off by default. When it's on, the other PC's keyboard and mouse also work in installers and apps running as administrator, in UAC prompts ("Do you want to allow…") and on the lock screen. Windows blocks a normal app from all three.
  - **How it works.** It's the same approach as Mouse Without Borders' "Use service". The MSI registers a Windows service, *Trovul input helper*, which stays stopped until the option is turned on. The service then starts a helper in your session with system rights. The helper replays the other PC's input on whichever desktop has it, and only accepts the Trovul app itself as a client.
  - **The trade-off.** It's stated next to the switch: while it's on, any program running on this PC could also answer UAC prompts.
  - **Installer.** Needs the .msi installer (the per-user .exe can't register a service). Uninstalling removes the service.

## 0.7.5

### Keyboard & Mouse
- **Drags on the other PC stay there too.** Dragging a tab or a window on the other PC with this PC's mouse, toward this PC's screen in the middle, now carries on to the other PC's next screen instead of pulling the pointer back here. 0.7.4 only did this for drags on the PC with the mouse.
- **The first key after crossing goes to the right PC.** When the pointer crosses, every key goes to the PC it's on, the first press included. Keys the previous PC saw go down are let go of there at once. Before, a key considered "still held" (e.g. Win, after a key-up another app or Windows kept to itself) opened the Start menu on the previous PC the first time and only reached the right PC the second time. Letting go of Win or Alt taps an unassigned key first, so it doesn't open Start or a menu bar.
- **Corners** are also kept when the pointer is on the other PC.

## 0.7.4

### Keyboard & Mouse
- **Drag windows past the other PC's screen.** A drag never goes to the other PC; it now carries on to this PC's next screen in the same direction. With the other PC's screen drawn between two of yours, a window can still be dragged from one to the other.
- **The keyboard follows the pointer.** When the pointer arrives on a screen, the window under it gets the keyboard, unless you were already typing on that screen. Windows itself only moves the keyboard on a click, so typing used to land in a window on the previous screen. It can be turned off.
- **Game mode.** The pointer stays on this PC while a game is in front, so a flick to the edge of the screen never throws it out of the game or minimizes it. A game is recognized when:
  - it holds the cursor inside its window;
  - it runs in exclusive full screen;
  - it runs full screen with the cursor hidden;
  - or it runs full screen and isn't a browser, video player or slideshow.
- **Scroll Lock** keeps the pointer on the PC it's on, as in Synergy and Barrier. The Scroll Lock key always stays on the PC you're sitting at.
- **Corners stay on this PC.** The pointer doesn't switch PCs in the corners of a screen (about a title bar's height), so the close button of a maximized window and the clock stay easy to hit. It can be turned off.
- **Fewer keys typed on the wrong PC.** A key or button whose release Windows never reported (lock screen, UAC, a stalled hook) no longer counts as held. Before, its next press was typed on the previous PC and every move was treated as a drag.
- **Admin windows.** Windows doesn't let a normal app type into an app running as administrator. When the other PC's keyboard reaches one, Trovul now says so once, instead of the keys just vanishing.

## 0.7.3

### Product
- **Shared clipboard.** Copy text or an image on one PC and paste it on the other. It's on the Keyboard & Mouse page, on by default, and works whenever the PCs are linked.
  - **Images.** Screenshots and pictures travel as PNG, in pieces, so the shared mouse stays smooth while a big image crosses. Apps get both a bitmap and a PNG (transparency kept).
  - **Private content stays.** Content that password-manager apps mark as private (KeePass, the 1Password and Bitwarden desktop apps…) is never sent. Browser extensions can't mark it, so a password copied from one does cross, like any text. Each PC's own switch decides what it sends and accepts.
  - **Order and reliability.** A newer copy always wins, even when an older image is still being prepared or downloaded. A copy the other PC refused (switch off, older version) is offered again the next time. Turning the switch off stops an image that's on its way.
  - **Protocol 2.3.** Adds `ClipboardOffer` and `GetClipboardChunk`.
- **Scenes.** Save the mix you have now as a scene ("Game", "Stream", "Meeting"…) and bring it back with one click, from the Mixer or the tray mixer. A scene holds both PCs: master, every app's volume, mute and output device, and the volume of the other PC's sound. Apps are matched by name, so a scene works whenever the same apps are open. "⋯" on a scene updates it with the current mix or deletes it. Up to 12 scenes.
- **First impressions.** Until the first link, Home shows what linking brings (sound, keyboard and mouse, mixer, clipboard). After an update, a "What's new" dialog shows the new features once, with "Show me" links.
- **Tray mixer.** A left click on the tray icon opens a small mixer above it, like the Windows volume flyout: where the sound goes (Hear / Send / Off), this PC's Master and the other PC's sound as dials, and every app as a slider with its live level and mute. It closes when you click elsewhere or press Esc. "Open Trovul" is in its corner and in the tray menu.

## 0.7.2

### Product
- **Mixer.** The Audio page is now a mixer laid out like an audio interface (think GoXLR), one deck per PC.
  - **Strips.** Every channel has a scribble strip, a big rotary dial with an LED ring for the live level, a Mute button and its output device.
  - **Dials.** Drag up or down (Shift for fine steps), scroll, or use the arrow keys; double-click goes back to 100 %. Scrolling the page over a dial scrolls the page.
  - **Buses.** Each PC has its Master and Trovul's stream: **Return** (the other PC's sound playing here, with its volume, mute and output) or **Send** (this PC's sound going out, with its level). The other PC's return can be set from this one.
  - **Output per app.** Send Discord to the headset and the game to the speakers, on either PC, as in Windows' "App volume and device preferences". Windows remembers the choice per app.
  - **Every device.** Apps playing on any output show up, not only those on the default one. Trovul's own "Incoming" entry is gone; its stream has its own strip.
  - **Where it went.** The sidebar says Mixer. Direction, test sound and connection quality are on the same page.

### Under the hood
- **Protocol 2.2.** Adds `GetOutputDevices`, `SetAppOutput`, `SetIncoming` and `SetPlaybackDevice`, plus the stream and output fields in mixer snapshots. A 0.7.1 peer still shows its master and apps, without the new controls.
- **Faster snapshots.** Sessions are found once a second and read on every poll: a snapshot takes about 1 ms instead of 15–25 ms.
- **Smoother input.** The peer's requests run on their own thread, so a mixer snapshot or an update chunk never holds up the shared mouse and keyboard.

## 0.7.1

### Product
- **Cleaner interface.** Same look, simpler structure, in the style of System Settings.
  - **Four places.** Home, Audio, Keyboard & Mouse and Settings. "PCs" is gone: nearby PCs appear on Home when nothing is connected, and the name and paired PCs are in Settings. Unbuilt features are hidden.
  - **Home.** The two PCs with one status line (and Disconnect), the audio direction and volume, and the keyboard & mouse switch.
  - **Grouped lists.** Pages are lists of "label → control" with short text. Details, connect by IP, troubleshooting and the activity log only show when asked for.
  - **Audio.** Now Sound and Apps. The Devices tab duplicated Windows' own settings and is gone. Connection quality is one line, and on a bad network it offers "Use Stable".

## 0.7.0

**Install 0.7.0 by hand on both PCs once.** From then on, updates pass from one linked PC to the other.

### Product
- **Updates over the link.** Install a new Trovul on one PC; the linked PC shows *"Trovul x.y.z is on <PC> — Update now"* and takes the installer straight from it, with no internet needed.
  - **How it works.** The MSI keeps a copy of itself in `%ProgramData%\Trovul\updates`, locked to SYSTEM and Administrators. Trovul only shares that copy if its owner is SYSTEM or Administrators.
  - **Security.** The offer is signed with the pairing token (HMAC-SHA256), so nobody else on the network can slip in an installer. The file must match the offered SHA-256 and be an MSI. Installing still asks for Windows' permission.
  - **Install.** msiexec runs in passive mode and reopens Trovul when it's done. If it fails or is cancelled, the old Trovul starts again.
  - **Protocol 2.1.** Adds `GetUpdateInfo` and `GetUpdateChunk` (256 KB pieces on the control link).
- **Keyboard & Mouse sharing.** One mouse and keyboard for both PCs, in any screen arrangement.
  - **Arrangement.** Drag every monitor of both PCs to where it really is on your desk. Screens snap edge to edge, and each monitor moves on its own, so the other PC's screen can sit *between* two screens of this PC. The pointer then goes A → C → B instead of jumping past both of this PC's screens.
  - **How the pointer moves.** Movement within a screen is left to Windows. At every edge, the drawn arrangement decides where the pointer goes, even where Windows thinks two screens touch. The arrangement is synced between the PCs, and the newest edit wins.
  - **Opt-in.** Both PCs must turn it on. Either PC's mouse can then reach the other.
  - **Safety.** Ctrl+Alt+Esc brings the pointer home. The other PC's own mouse takes it back. A dropped link returns it at once and releases every key. Drags never cross. If Windows drops the input hook, the invisible overlay steps aside.
  - **Protocol 2.1.** Input travels on the paired control link as one-way frames.
- **New interface.** The whole UI was redesigned around the link between two PCs and the features that run over it, in the look of trovul.com: near-black background, violet → cyan → emerald accents, Inter and JetBrains Mono (bundled, so it works offline) and Lucide icons.
  - **Layout.** A sidebar shows the link status and has sections for Overview, PCs, Features (Audio, plus Keyboard & Mouse and Screen marked *Soon*) and Settings. Narrow windows get an icon rail. The title bar is dark, and the window is wider by default.
  - **Overview.** The two PCs with an animated link: particles flow in the audio's direction at the audio's level, with live delay and bitrate. Also a quick Audio card and recent activity. When no PC is linked, it becomes a setup flow with nearby PCs.
  - **Audio.** Stream (direction cards, volume with the live level inside the slider, stream quality tiles, latency), Mixer (per-app rows for both PCs) and Devices.
  - **PCs.** This PC (name, address, network profile), the linked PC, nearby PCs, connect by IP and paired PCs.
  - **Pairing prompt.** Now a dialog with a 45 s countdown. It never pre-selects Accept, so an Enter typed into another app can't pair a PC.
  - **For developers.** New features plug in through a feature registry (`src/app/features.ts`).

## 0.6.0 — test build (0.6.0-beta.1)

**Both PCs must run 0.6.0.** The wire protocol is now v2; older builds show up in the peer list as "Update needed".

### Product
- **Home tab.** Shows both PCs with the audio flow animated in the active direction. One click on either PC ("Hear the other PC here", "Send this PC's audio", "Off") configures both PCs, and if the other PC can't follow, the change is rolled back.
- **Set and forget.**
  - **Pairing.** Uses a persistent device id plus a shared token, so a new IP address (DHCP) doesn't bring the prompt back. The token crosses the network only once, at pairing. Every later connection proves it with an HMAC-SHA256 answer to a fresh challenge, so a sniffed handshake can't be replayed to impersonate a paired PC.
  - **Reconnect.** Automatic after reboots, sleep, Wi-Fi drops or app restarts, and the audio direction is restored. Only one side dials, so the two PCs don't collide.
  - **Startup.** Trovul starts hidden in the tray when launched at Windows sign-in.
- **Incoming audio controls.** Volume and mute with a 10 ms glide (no clicks), a level meter, and a choice of output device. Bluetooth "Hands-Free" outputs, a headset's phone-quality call mode, are labelled in the list, with a warning when one is in use.
- **Test sound.** The sending PC plays a chime that travels the real path to the other PC, as an end-to-end check.
- **Voice-chat ducking.** While Discord, Teams, Zoom, Slack, TeamSpeak or another call app on the receiving PC plays someone's voice, the other PC's audio drops to 30%. It holds through short pauses and fades back when they stop. Home shows a "Lowered for voice chat" badge meanwhile. On by default; Settings → General turns it off. Call-app sessions are found once a second on every output device, and their meters are read every 50 ms.
- **Global shortcuts.** Ctrl+Alt+M mutes incoming audio and Ctrl+Alt+T turns audio on and off. Settings shows when another app already uses a combination.
- **Latency modes.** Low (≈30 ms), Balanced (≈50 ms) and Stable (≈120 ms); the jitter buffer still adapts within each.
- **Settings tab.** Start with Windows, start in the tray, auto-reconnect, notifications, shortcuts, latency, paired PCs (forget), Companion API, and an "Open log folder" button.
- **Copy support info.** One click in Settings → About copies a support report to the clipboard: versions, connection, audio pipeline counters, settings and the last 120 log lines. Pairing and Companion tokens are never included.
- **Tray quick actions.** Hear, send or stop audio, and mute incoming audio.
- **Tray status badge.** A dot on the tray icon shows the state without opening the window: green when connected, blue while audio flows, amber while waiting to reconnect.
- **Changes from the other PC are announced.** When the other PC starts, swaps or stops the audio, a toast and a log line say what changed.
- **First run.** A "how it works" card, and troubleshooting tips when no PC shows up after 20 s (guest Wi-Fi, firewall, connect by IP).
- **Public network warning.** Windows hides a PC whose network is marked Public. When the LAN adapter is on a Public network, Home says so and opens the matching Windows settings page (Wi-Fi or Ethernet). Virtual adapters (Hyper-V, WSL, VPNs) don't trigger it. The support report includes the network profile.
- **Name this PC.** Settings → General sets the name the other PC sees ("Living room" instead of "DESKTOP-4F7Q2KD"); discovery picks it up within seconds.
- **Connection quality indicator.** Excellent / Good / Unstable, with a hint to switch to Stable when the network drops audio.
- **Installer.** The MSI adds a "Trovul (private networks)" firewall rule and removes Trovul's block rules. That means no firewall prompt on first launch, and an earlier "Cancel" on that prompt no longer blocks discovery. Allow rules the user made, for example for public networks, are kept. Uninstalling removes every rule for Trovul. The NSIS .exe stays per-user, as before, so it can't change the firewall. Publisher, description and category are set, and the window has a minimum size.

### Audio
- **Rewritten receive pipeline.** Resampling works on whole stereo frames (channels no longer bleed into or swap with each other), keeps its interpolation state across packets, and feeds an adaptive jitter buffer: it starts at 50 ms, grows after underruns caused by late packets and relaxes back toward 30 ms on a clean link. Playback speed is steered by a critically-damped PI controller that absorbs clock drift. The previous controller aimed at ~0.5 s of buffering and oscillated between roughly 50 ms and 950 ms.
- **Any speaker layout.** The sender downmixes mono … 7.1 captures to stereo, and the receiver maps stereo onto any output layout. Float and 16/24/32-bit integer mix formats are supported. Surround devices on either side used to produce garbled audio.
- **Leaner packets.** PCM16 stereo in 5 ms packets under 1 KB (no IP fragmentation, ~1.6 Mbit/s) instead of float PCM in ~3.8 KB fragmented datagrams (~3 Mbit/s). Packets carry a per-connection session id and sequence numbers, so loss, reordering and stream restarts are detected.
- **Sessions own their threads.** Stopping or switching modes joins the old session's threads before returning, so restarts no longer leak threads or race the next session for the UDP port.
- **Device changes and failures.** Capture and playback follow changes of the Windows default device, and reopen with exponential backoff after device errors (Bluetooth profile switches, USB unplug, resume from sleep).
- WASAPI "silent" buffers are honoured. The mix format and device-id strings are now freed; they leaked on every call.

### Connection & security
- **Persistent control link.** One TCP connection per pairing carries length-prefixed JSON frames: requests in both directions, heartbeats (a vanished peer is detected in about 7 s) and a goodbye message. It replaces opening a new TCP connection per request (10 per second while the mixer was open), and fixes truncated responses: large mixer lists used to be read with a single `read()`.
- **Only the paired PC is trusted.** Mixer control only works over the approved link, and audio is only accepted from the paired PC's address with the link's session id. Before, anyone on the LAN could list or mute apps and inject audio.
- Disconnecting (button, tray, peer, timeout) now always stops audio streaming.
- **No more false auto-disconnects.** When the sender goes quiet, the connection stays up: peer liveness now comes from the control-link heartbeat instead of audio packets.
- **Prompts can't be missed.** Connection requests bring the window to the front, and requests that expire after 45 s close their prompt.
- Paired PCs are remembered in `%APPDATA%/com.trovul.app/paired_devices.json` and reconnect without a prompt. The old IP-based `trusted_peers.json` is no longer read, so approve the other PC once after updating.

### Companion API
- **Off by default.** Enable it under Settings → Companion API. Clients must connect with `?token=…`; the token is shown in the app and can be regenerated.
- Master volume and mute are implemented, `remote: true` forwards the command to the connected PC, and master peaks are real.

### App
- **Responsiveness.** Commands that touch COM or the network run off the UI thread. Polling never overlaps and pauses while the window is hidden, and icons are fetched once instead of with every 100 ms poll.
- **Mixer.** The remote mixer has a working master strip, and the local master meter shows the real peak. The per-app remote output selector and the PAN knobs, which did nothing, were removed. When no PC is connected, the Mixer links to Home to connect one.
- **Keyboard.** Buttons, inputs and toggles show a focus ring when reached with Tab.
- **Diagnostics card.** Shows buffer latency, drift, loss, underruns and bitrate, and hints when the two PCs' modes don't match.
- Errors show up as toasts instead of being swallowed, and the playback device choice persists across restarts.
- **Single-instance lock.** A second launch focuses the running window instead of starting a copy that can't bind its ports.
- **Resume from sleep.** A wall-clock-gap watchdog detects it and rebuilds the audio device handles.
- **Logging.** Logs go to `%LOCALAPPDATA%/com.trovul.app/logs` with size-based rotation (5 MB per file, 5 old files kept), with no log line per request.
- Discovery broadcasts to every local subnet instead of a hard-coded `192.168.0.255`.

### Cleanup & tooling
- **Repository hygiene.** Removed obsolete local references, generated logs, and unused webview plugin permissions.
- **Tests.** Unit tests cover the DSP, the wire format and the control link (`cargo test`). A hardware smoke test that makes no sound runs with `cargo test -- --ignored hardware`.
- **CI.** `.github/workflows/ci.yml` runs rustfmt, clippy, the tests and the frontend build on Windows. `release.yml` builds the installer on `v*` tags.
