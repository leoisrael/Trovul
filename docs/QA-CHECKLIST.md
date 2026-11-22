# Release smoke test

Run this before every release, on two Windows PCs (A and B) on the same network. It takes about 20 minutes. Automated tests (`cargo test`) cover the DSP, the wire format, the control link and pairing; this list covers what needs real hardware, two machines and a human ear.

## Setup

- [ ] Uninstall older builds on both PCs, or install the new MSI over them (same version upgrades in place).
- [ ] Install on both PCs with the **MSI**, not `tauri dev` or the .exe: only the MSI adds the firewall rule.
- [ ] No Windows Firewall prompt appears on first launch.
- [ ] Settings → About shows the expected version on both PCs.

## Discovery and pairing

- [ ] Each PC lists the other under Nearby PCs within a few seconds.
- [ ] Set a network to Public (Windows Settings → Network): Home shows the Public-network warning, and "Open network settings" opens the right page. Set it back to Private: the warning disappears within about 5 s.
- [ ] Settings → Name of this PC: rename A while disconnected. B's Nearby PCs list shows the new name within a few seconds. A connected PC shows it after reconnecting.
- [ ] Connect from A: B shows the request (window comes to the front), Accept → both show Connected.
- [ ] Disconnect, connect again: no prompt this time (paired). Settings → Paired PCs lists the other PC.
- [ ] Decline a request: A shows "declined". A second request within 60 s is refused without a prompt.

## Audio

- [ ] On A: "Hear B here". B's audio plays on A. B shows it is sending. Delay feels live, and the connection-quality line says Excellent or Good.
- [ ] Test sound: a two-note chime plays through the link.
- [ ] Incoming volume and Mute: no clicks. Ctrl+Alt+M mutes from inside a full-screen game.
- [ ] Ctrl+Alt+T turns the audio off, then back on in the same direction.
- [ ] Swap the direction from B's window: A shows a toast saying B changed it.
- [ ] Output device: switch to another device while playing; audio moves without reconnecting.
- [ ] Bluetooth headset (if available): open its mic in any app. The Hands-Free output is labelled "call quality", and a warning shows when it's in use.
- [ ] Unplug or switch the Windows default output while playing: audio recovers on its own within a few seconds.
- [ ] Surround or odd devices (if available): a 5.1/7.1 output or a USB headset plays correctly on both sides.

## Send only chosen apps (Windows 11 sender)

- [ ] On the Windows 11 PC, choose Send. On its Send strip pick **Apps**: nothing plays on the other PC until an app is switched on with ➜.
- [ ] Play music in Spotify and a video in the browser there. Switch on ➜ for Spotify only: the other PC hears Spotify, not the browser. The guide line lists Spotify.
- [ ] Lower Spotify's bar under ➜: quieter on the other PC only. Lower its dial: quieter on the sending PC (check whether the other PC follows too, and note it).
- [ ] Mute Spotify on the sending PC: note whether the other PC still hears it.
- [ ] Close and reopen Spotify: it's sent again within a second, without touching anything.
- [ ] Do the same from the other PC's Mixer (the remote deck): the switches work there too.
- [ ] Back to **All**: everything plays again, no reconnect.
- [ ] On a Windows 10 sender, the Send strip says "All sound" and has no switches.

## Follow the pointer

- [ ] On the PC with the headset (hearing the other PC), turn on "Follow pointer" in the Mixer. Play music on both PCs. With the pointer on this PC, the other PC's music is quieter. Move the pointer to the other PC: this PC's music drops and the other's comes back. The decks show In focus / Quieter.
- [ ] The dials of this PC's apps don't move while they're lowered. Windows' volume mixer shows the same values as before.
- [ ] Quit Trovul from the tray while the pointer is on the other PC: this PC's apps come back to full volume on the next start.

## Pop-ups and files

- [ ] Link the PCs: a "Linked to …" card appears bottom-right and fades. Pull the network cable on the other PC: "Lost the link…". Neither takes the focus from the window you're typing in.
- [ ] In a game in exclusive full screen, no card appears.
- [ ] Copy 3 small files in Explorer on A, paste in a folder on B: they're there, with the same names and content. A folder with subfolders too.
- [ ] Copy a 1 GB+ file on A: B shows "… copied … Receive". Receive: the progress bar moves, Cancel stops it. Receive again and let it finish: "Show in folder" opens DownloadsTrovul with it selected, and Ctrl+V pastes it.
- [ ] Copy a file on B that came from A: nothing bounces back to A.

## Stream Deck (plugin at streamdeck/dist)
- [ ] With Settings → Stream Deck & controllers on, double-click the `.streamDeckPlugin`: the Trovul category appears in Stream Deck.
- [ ] Scene key: pick a scene in its settings; pressing it applies the scene on both PCs (✓ on the key).
- [ ] Where the sound goes: "Cycle" goes Hear → Send → Off and the key follows changes made in the app.
- [ ] Mute: master, the other PC's sound, an app by name, and "on the other PC"; the key turns red and follows mutes made in the app.
- [ ] Stream Deck + dial: turning changes the volume in the Mixer, pressing mutes, the strip shows the level.
- [ ] Follow the pointer key lights up with the setting.
- [ ] Turn "Allow controllers" off: a key press shows the ⚠ alert; on again: the keys come back within a few seconds.

## Keyboard & mouse details

- [ ] Using the mouse of the PC in the middle, drag a tab on the other PC from its left screen toward the middle: it jumps to the other PC's right screen, still dragging. Without a button held, the pointer comes to the middle as usual.
- [ ] Move the pointer to the other PC and press Win once: Start opens there, not on the PC with the keyboard. Same for Alt+Tab and Ctrl+C.
- [ ] Hold Shift, cross to the other PC, let go: Shift isn't stuck on either PC.
- [ ] Layout A | C | B (the other PC's screen between two of this PC's). Drag a window by its title bar from A toward C: it jumps to B and keeps following the mouse; drop it there. Also from B back to A.
- [ ] Click in a window on A, move to the other PC's screen and type without clicking: the window under the pointer there gets the text. Come back to B and type: B's window gets it. Come back to A, where you were typing: A's window still has the keyboard.
- [ ] With "Keyboard follows the pointer" off, typing stays in the window last clicked on each PC.
- [ ] Game mode: start a game (full screen or borderless) and flick the mouse against the edge toward the other PC: the pointer stays in the game. Alt+Tab to the desktop: the pointer crosses again.
- [ ] Full-screen YouTube in a browser: the pointer still crosses.
- [ ] Scroll Lock on: the pointer can't cross, from either PC. Off: it crosses again.
- [ ] Corners: move fast to the close button of a maximized window at the corner next to the other PC: the pointer stops there. It crosses anywhere else along the edge.
- [ ] On the controlled PC, open an admin app (e.g. Task Manager) and type into it from the other PC: a notice explains why it doesn't work.

## Admin windows, UAC and the lock screen (MSI install)

- [ ] On the PC being controlled, turn on "Admin windows, UAC and lock screen": it says "On" within a few seconds. `sc query TrovulInput` shows RUNNING. Off stops it.
- [ ] From the other PC, run an installer on it: click "Yes" on the UAC prompt and click through the installer.
- [ ] Open Task Manager as admin there and type into its search box.
- [ ] Lock that PC (Win+L) and type its password from the other PC.
- [ ] With the option off, typing into an admin window shows the notice that suggests turning it on.
- [ ] Uninstall Trovul: the service is gone (`sc query TrovulInput` → not found).

## Shared clipboard

- [ ] Copy text on A, paste on B (Notepad, a browser field). Accents and emoji arrive intact. Keyboard & Mouse shows "… from A · just now".
- [ ] Take a screenshot on B (Win+Shift+S) and paste it on A in Paint and in a browser/Discord message.
- [ ] Copy a large image (a 4K screenshot) while using the shared mouse: the pointer stays smooth.
- [ ] Copy a password from a password-manager app (KeePass, 1Password or Bitwarden desktop): it doesn't reach the other PC. (Browser extensions can't mark content private; theirs crosses like any text.)
- [ ] Take a screenshot and copy text right after it: the other PC ends up with the text, not the screenshot.
- [ ] Turn "Share clipboard" off on B, copy "abc" on A, turn it back on, copy "abc" again: it arrives.
- [ ] Turn "Share clipboard" off on B: copies from A don't arrive on B, and B's copies don't reach A.
- [ ] Copy on A, paste on B, copy the same text on B: nothing bounces back and forth.

## Mixer

- [ ] The Mixer shows both PCs: Master, Return (on the PC that hears) or Send (on the PC that sends), and every app playing, including apps on a non-default output. There is no "Incoming" or "trovul" strip.
- [ ] Dials: dragging up/down, scrolling, the arrow keys and PageUp/PageDown change the volume; double-click goes to 100 %. Windows' volume mixer shows the same values. Scrolling the page with the pointer passing over a dial doesn't change it.
- [ ] Mute on each strip lights red and silences only that app (or the master / the return).
- [ ] Output per app: send a browser tab playing music to another device on this PC, then on the other PC. The sound moves within a second, and Windows Settings → "App volume and device preferences" shows the same choice. "Default" puts it back.
- [ ] The other PC's Return: while sending, change its volume, mute and output from this PC. Its window and tray follow.
- [ ] With the Mixer open on the PC being controlled with the shared mouse, the pointer stays smooth.
- [ ] Scenes: set a mix (volumes, a mute, an app on another output, on both PCs), "Save mix" as "Game". Change everything, click "Game": it all comes back, on both PCs, and the chip glows. "⋯ → Update" saves the new mix, "⋯ → Delete" removes it. The tray mixer shows the same scenes.
- [ ] With the other PC on 0.7.1: its master and apps still work, without output pickers or a stream strip.

## Voice-chat ducking

- [ ] A receives B's audio (music or a game). Join a Discord or Teams call on A, or play a voice message: B's audio drops while someone talks, and Home shows "Lowered for voice chat". It comes back about 1.5 s after they stop.
- [ ] Settings → Lower the other PC during calls → off: no more ducking.

## Resilience

- [ ] Restart B: A shows Reconnecting, then both reconnect and the audio direction is restored, with no prompt.
- [ ] Sleep and wake one PC: the same thing happens.
- [ ] Pull the network cable or turn Wi-Fi off for 30 s: the link drops in about 7 s ("stopped responding — audio stopped"), then reconnects when the network is back.
- [ ] Disconnect on purpose: neither PC reconnects by itself.
- [ ] Quit from the tray on B: A reconnects when B starts again.

## Tray and startup

- [ ] Left click on the tray icon: the tray mixer opens above the icon (also with the taskbar on top or on a side, and on a 125–150 % display). Dials, sliders, mute and Hear / Send / Off work; the main window follows the same changes.
- [ ] Click elsewhere, press Esc or click the icon again: it closes. The expand button opens the main window.

- [ ] The tray dot is green when connected, blue while audio flows, amber while reconnecting, and absent when not connected.
- [ ] Tray menu: hear, send, stop and mute work with the window hidden.
- [ ] Start with Windows is on: after signing in, Trovul starts hidden in the tray and reconnects.
- [ ] Closing the window hides it to the tray; launching Trovul again brings the same window back.

## Updates over the link

- [ ] Build a higher version, install it on A. Link with B: B shows "Trovul x.y.z is on A — Update now".
- [ ] Update now: progress bar, Windows' permission prompt, Trovul closes, installs and reopens on the new version, and reconnects.
- [ ] Cancel the permission prompt: the old Trovul starts again.
- [ ] B, now updated, can pass the same version on (its `%ProgramData%\Trovul\updates` holds the MSI).

## Support

- [ ] Settings → About → Copy support info: the pasted report has versions, the network profile, audio counters and the recent log, and no tokens.
- [ ] Open log folder opens `%LOCALAPPDATA%\com.trovul.app\logs`.

## Uninstall

- [ ] `wf.msc` → Inbound Rules shows "Trovul (private networks)" once, even after installing twice. Uninstall: the app is gone and the rule is removed.
