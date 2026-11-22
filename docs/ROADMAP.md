# Roadmap

What's planned beyond 0.7.x, in rough order.

## Next

### Drag files between the PCs
Copy and paste already works (0.7.8). Dragging is the next step: drag a file in Explorer to the other PC's screen and drop it there.
- **How.** An invisible drop window sits at the screen edge where the pointer would cross. It takes the drag's file list while the button is held.
- **Transfer.** The files go through the same pieces as clipboard files (`clipfiles.rs`).
- **Drop.** The other PC starts a drag with the received files under the pointer.
- **The one exception.** Today a drag never crosses to the other PC (`drag_past`); files would be the only kind that does.

### Mixer on the phone
Trovul serves a small web page with the mixer (dials, scenes, direction) on the local network, opened from a QR code in Settings.
- **Reuse.** It uses the companion API that the Stream Deck plugin uses (`get_state`, `adjust`, `apply_scene`).
- **Token.** The token travels in the QR code.

### Shortcuts for scenes, and an on-screen notice
Ctrl+Alt+1…9 apply scenes, and a small pop-up says which one. Scenes are already applied by the backend (`scenes::apply`), so this works with the window closed.

### Scenes that follow the app in front
"When Valorant is in front, use Game; when Zoom is, use Meeting." When the app leaves, the previous scene comes back.

## Later

- **Shared microphone.** Use the gaming PC's microphone on the streaming PC. Needs a virtual audio device (a signed driver).
- **The other PC's screen in a window.** Already in the feature registry as "Screen".
- **Three or more PCs, and Mac.**
