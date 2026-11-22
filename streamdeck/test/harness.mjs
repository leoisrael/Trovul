// End-to-end test of the built plugin without a Stream Deck: this script plays
// both the Stream Deck app (the WebSocket the SDK connects to) and Trovul's
// companion API, starts bin/plugin.js the way Stream Deck does, presses keys,
// turns a dial and checks what the plugin says to each side.
//
//   npm run build && node test/harness.mjs   (Node 24, like Stream Deck 7.1+)

import { spawn } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { WebSocketServer } from "ws";

const pluginDir = join(dirname(fileURLToPath(import.meta.url)), "..", "com.trovul.streamdeck.sdPlugin");
const TOKEN = "test-token-123";
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------- Fake Trovul ----------

const trovulState = {
  connected: true,
  peer_hostname: "israel-sil",
  direction: "receive",
  incoming: { volume: 0.8, muted: false },
  follow_pointer: false,
  send_apps_only: false,
  scenes: [{ id: "s1", name: "Game" }],
};
const targets = new Map(); // key → {volume, muted}
const toTrovul = [];
const trovul = new WebSocketServer({ port: 0 });
let tokenOk = false;
trovul.on("connection", (ws, req) => {
  tokenOk = new URL(req.url, "http://x").searchParams.get("token") === TOKEN;
  const send = (type, payload) => ws.send(JSON.stringify({ type, payload, timestamp: Date.now() }));
  send("state", trovulState);
  ws.on("message", (data) => {
    const { type, payload } = JSON.parse(String(data));
    toTrovul.push({ type, payload });
    if (type === "get_state") send("state", trovulState);
    if (type === "apply_scene") send("scene_applied", { id: payload.id, summary: "applied" });
    if (type === "set_direction") {
      trovulState.direction = payload.direction;
      send("state", trovulState);
    }
    if (type === "set_follow_pointer") {
      trovulState.follow_pointer = payload.on;
      send("state", trovulState);
    }
    if (type === "adjust") {
      const key = `${payload.target}|${payload.name ?? ""}|${payload.remote ? 1 : 0}`;
      const t = targets.get(key) ?? { volume: 0.5, muted: false };
      if (payload.delta !== undefined) t.volume = Math.min(1, Math.max(0, t.volume + payload.delta));
      if (payload.mute === "toggle") t.muted = !t.muted;
      targets.set(key, t);
      send("target", { target: payload.target, name: payload.name ?? null, remote: !!payload.remote, ...t });
    }
  });
});
await new Promise((r) => trovul.on("listening", r));
const companionFile = join(mkdtempSync(join(tmpdir(), "trovul-sd-")), "companion.json");
writeFileSync(companionFile, JSON.stringify({ port: trovul.address().port, token: TOKEN }));

// ---------- Fake Stream Deck ----------

const toDeck = [];
const deck = new WebSocketServer({ port: 0 });
await new Promise((r) => deck.on("listening", r));
let pluginSocket;
let registered = false;
deck.on("connection", (ws) => {
  pluginSocket = ws;
  ws.on("message", (data) => {
    const message = JSON.parse(String(data));
    if (message.event === "registerPlugin") registered = true;
    else toDeck.push(message);
  });
});

const info = {
  application: { font: "Segoe UI", language: "en", platform: "windows", platformVersion: "10.0.26100", version: "7.1.0.0" },
  colors: {
    buttonMouseOverBackgroundColor: "#464646FF",
    buttonPressedBackgroundColor: "#303030FF",
    buttonPressedBorderColor: "#646464FF",
    buttonPressedTextColor: "#969696FF",
    highlightColor: "#0078FFFF",
  },
  devicePixelRatio: 1,
  devices: [{ id: "dev1", name: "Stream Deck +", size: { columns: 4, rows: 2 }, type: 7 }],
  plugin: { uuid: "com.trovul.streamdeck", version: "0.1.0.0" },
};
const plugin = spawn(
  process.execPath,
  ["bin/plugin.js", "-port", String(deck.address().port), "-pluginUUID", "PLUGIN1", "-registerEvent", "registerPlugin", "-info", JSON.stringify(info)],
  { cwd: pluginDir, env: { ...process.env, TROVUL_COMPANION_FILE: companionFile }, stdio: ["ignore", "pipe", "pipe"] },
);
let pluginOutput = "";
plugin.stdout.on("data", (d) => (pluginOutput += d));
plugin.stderr.on("data", (d) => (pluginOutput += d));

const waitFor = async (what, test, ms = 3000) => {
  const start = Date.now();
  while (Date.now() - start < ms) {
    if (test()) return true;
    await sleep(25);
  }
  return false;
};
const sendDeck = (event, action, context, payload) =>
  pluginSocket.send(JSON.stringify({ event, action, context, device: "dev1", payload }));
const keyPayload = (settings) => ({ settings, coordinates: { column: 0, row: 0 }, controller: "Keypad", state: 0, isInMultiAction: false });
const dialPayload = (settings, extra = {}) => ({ settings, coordinates: { column: 0, row: 0 }, controller: "Encoder", ...extra });
const deckSaid = (event, context, test = () => true) => toDeck.some((m) => m.event === event && m.context === context && test(m.payload ?? {}));
const trovulGot = (type, test = () => true) => toTrovul.some((m) => m.type === type && test(m.payload ?? {}));

try {
  check("plugin registers with Stream Deck", await waitFor("register", () => registered));
  check("plugin connects to Trovul with the token from companion.json", await waitFor("trovul", () => tokenOk));
  await sleep(300);

  // Scene key
  sendDeck("willAppear", "com.trovul.streamdeck.scene", "k1", keyPayload({ sceneId: "s1" }));
  check("scene key shows the scene's name", await waitFor("title", () => deckSaid("setTitle", "k1", (p) => p.title === "Game")));
  sendDeck("keyDown", "com.trovul.streamdeck.scene", "k1", keyPayload({ sceneId: "s1" }));
  check("pressing it applies the scene in Trovul", await waitFor("apply", () => trovulGot("apply_scene", (p) => p.id === "s1")));
  check("and the key shows OK", await waitFor("ok", () => deckSaid("showOk", "k1")));

  // Mute key for an app
  sendDeck("willAppear", "com.trovul.streamdeck.mute", "k2", keyPayload({ target: "app", name: "Discord" }));
  check("mute key asks Trovul for Discord's state", await waitFor("query", () => trovulGot("adjust", (p) => p.name === "Discord" && p.mute === undefined)));
  check("mute key is titled with the app", await waitFor("title", () => deckSaid("setTitle", "k2", (p) => p.title === "Discord")));
  sendDeck("keyDown", "com.trovul.streamdeck.mute", "k2", keyPayload({ target: "app", name: "Discord" }));
  check("pressing it toggles Discord's mute", await waitFor("toggle", () => trovulGot("adjust", (p) => p.name === "Discord" && p.mute === "toggle")));
  check("and the key turns to its muted state", await waitFor("state", () => deckSaid("setState", "k2", (p) => p.state === 1)));

  // An app on the other PC
  sendDeck("willAppear", "com.trovul.streamdeck.mute", "k5", keyPayload({ target: "app", name: "Spotify", remote: true }));
  await sleep(100);
  sendDeck("keyDown", "com.trovul.streamdeck.mute", "k5", keyPayload({ target: "app", name: "Spotify", remote: true }));
  check("a key can mute an app on the other PC", await waitFor("remote", () => trovulGot("adjust", (p) => p.name === "Spotify" && p.remote === true && p.mute === "toggle")));

  // Volume dial (Stream Deck +)
  sendDeck("willAppear", "com.trovul.streamdeck.volume", "d1", dialPayload({ target: "master" }));
  check("dial shows the master level on the touch strip", await waitFor("feedback", () => deckSaid("setFeedback", "d1", (p) => p.title === "Master")));
  sendDeck("dialRotate", "com.trovul.streamdeck.volume", "d1", dialPayload({ target: "master" }, { ticks: 3, pressed: false }));
  check("turning it 3 ticks raises the master by 6 %", await waitFor("rotate", () => trovulGot("adjust", (p) => p.target === "master" && Math.abs((p.delta ?? 0) - 0.06) < 1e-9)));
  sendDeck("dialDown", "com.trovul.streamdeck.volume", "d1", dialPayload({ target: "master" }));
  check("pressing the dial mutes", await waitFor("dialmute", () => trovulGot("adjust", (p) => p.target === "master" && p.mute === "toggle")));

  // Direction key
  sendDeck("willAppear", "com.trovul.streamdeck.direction", "k3", keyPayload({ direction: "cycle" }));
  check("direction key shows the current direction", await waitFor("dir", () => deckSaid("setTitle", "k3", (p) => p.title === "Hear")));
  sendDeck("keyDown", "com.trovul.streamdeck.direction", "k3", keyPayload({ direction: "cycle" }));
  check("pressing it moves on to Send", await waitFor("send", () => trovulGot("set_direction", (p) => p.direction === "send")));
  check("and the key follows Trovul's new state", await waitFor("dir2", () => deckSaid("setTitle", "k3", (p) => p.title === "Send")));

  // Follow the pointer
  sendDeck("willAppear", "com.trovul.streamdeck.follow", "k4", keyPayload({}));
  sendDeck("keyDown", "com.trovul.streamdeck.follow", "k4", keyPayload({}));
  check("follow-the-pointer key turns it on", await waitFor("follow", () => trovulGot("set_follow_pointer", (p) => p.on === true)));
  check("and lights up", await waitFor("followstate", () => deckSaid("setState", "k4", (p) => p.state === 1)));

  // Trovul not running: keys alert instead of pretending.
  for (const client of trovul.clients) client.terminate();
  await sleep(300);
  sendDeck("keyDown", "com.trovul.streamdeck.scene", "k1", keyPayload({ sceneId: "s1" }));
  check("with Trovul gone, a key shows an alert", await waitFor("alert", () => deckSaid("showAlert", "k1")));
} finally {
  plugin.kill();
  trovul.close();
  deck.close();
}

const failed = results.filter((r) => !r.ok).length;
console.log(`\n${results.length - failed}/${results.length} passed`);
if (failed) {
  console.log("\nPlugin output:\n" + pluginOutput.slice(-3000));
  process.exit(1);
}
process.exit(0);
