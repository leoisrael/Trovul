// Writes the plugin's SVG icons (node scripts/icons.mjs). Key images are 144 px
// with the icon in the upper part, leaving room for the title at the bottom.
// The plugin icon itself must be PNG (imgs/plugin.png, @2x): rendered once
// from logo(288, false) and committed.

import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "com.trovul.streamdeck.sdPlugin", "imgs");

const ICONS = {
  scene:
    '<path d="M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z"/>',
  direction: '<path d="M8 3 4 7l4 4"/><path d="M4 7h16"/><path d="m16 21 4-4-4-4"/><path d="M20 17H4"/>',
  sound:
    '<path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"/><path d="M16 9a5 5 0 0 1 0 6"/><path d="M19.364 18.364a9 9 0 0 0 0-12.728"/>',
  muted:
    '<path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"/><line x1="22" x2="16" y1="9" y2="15"/><line x1="16" x2="22" y1="9" y2="15"/>',
  volume: '<circle cx="12" cy="12" r="8"/><path d="m12 12 4-4"/><path d="M12 2v1"/><path d="M22 12h-1"/><path d="M2 12h1"/>',
  follow:
    '<circle cx="12" cy="12" r="10"/><line x1="22" x2="18" y1="12" y2="12"/><line x1="6" x2="2" y1="12" y2="12"/><line x1="12" x2="12" y1="6" y2="2"/><line x1="12" x2="12" y1="22" y2="18"/>',
};

export const logo = (size, mono) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="${size}" height="${size}">
  <defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0%" stop-color="${mono ? "#fff" : "#a78bfa"}"/><stop offset="55%" stop-color="${mono ? "#fff" : "#22d3ee"}"/><stop offset="100%" stop-color="${mono ? "#fff" : "#34d399"}"/></linearGradient></defs>
  ${mono ? "" : '<rect width="100" height="100" rx="22" fill="#0b0b12"/>'}
  <path d="M75 20 A35 35 0 0 1 75 80" fill="none" stroke="url(#g)" stroke-width="10" stroke-linecap="round"/>
  <path d="M25 80 A35 35 0 0 1 25 20" fill="none" stroke="url(#g)" stroke-width="10" stroke-linecap="round" opacity="0.55"/>
  <circle cx="40" cy="36" r="8" fill="${mono ? "#fff" : "#a78bfa"}" opacity="0.75"/>
  <circle cx="60" cy="64" r="8" fill="${mono ? "#fff" : "#22d3ee"}"/>
</svg>
`;

/** Monochrome list icon (Stream Deck tints it). */
const actionIcon = (paths) =>
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${paths}</svg>\n`;

/** A key: dark face, the icon up top in `color`, a soft glow when `lit`. */
const keyImage = (paths, color, lit) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 144 144" width="144" height="144">
  <defs><radialGradient id="glow" cx="50%" cy="38%" r="55%"><stop offset="0%" stop-color="${color}" stop-opacity="${lit ? 0.35 : 0.12}"/><stop offset="100%" stop-color="${color}" stop-opacity="0"/></radialGradient></defs>
  <rect width="144" height="144" fill="#0b0b12"/>
  <rect width="144" height="144" fill="url(#glow)"/>
  <g transform="translate(44 22) scale(2.33)" fill="none" stroke="${color}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${paths}</g>
</svg>
`;

const files = {
  "category.svg": logo(28, true),
  "actions/scene.svg": actionIcon(ICONS.scene),
  "actions/direction.svg": actionIcon(ICONS.direction),
  "actions/mute.svg": actionIcon(ICONS.muted),
  "actions/volume.svg": actionIcon(ICONS.volume),
  "actions/follow.svg": actionIcon(ICONS.follow),
  "keys/scene.svg": keyImage(ICONS.scene, "#a78bfa", true),
  "keys/direction-off.svg": keyImage(ICONS.direction, "#6c6c82", false),
  "keys/direction-on.svg": keyImage(ICONS.direction, "#22d3ee", true),
  "keys/sound-on.svg": keyImage(ICONS.sound, "#34d399", true),
  "keys/sound-muted.svg": keyImage(ICONS.muted, "#f87171", true),
  "keys/follow-off.svg": keyImage(ICONS.follow, "#6c6c82", false),
  "keys/follow-on.svg": keyImage(ICONS.follow, "#22d3ee", true),
};

for (const [name, body] of Object.entries(files)) {
  const path = join(root, name);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, body);
}
console.log(`wrote ${Object.keys(files).length} icons`);
