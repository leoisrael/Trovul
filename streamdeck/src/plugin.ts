// Trovul for Stream Deck: scenes, where the sound goes, mute and volume for
// both PCs, follow the pointer. Talks to Trovul through its companion API
// (src/trovul.ts).

import streamDeck, {
  type DialDownEvent,
  type DialRotateEvent,
  type KeyAction,
  type KeyDownEvent,
  SingletonAction,
  type WillAppearEvent,
  type Action,
  type DidReceiveSettingsEvent,
  type SendToPluginEvent,
} from "@elgato/streamdeck";
import type { JsonObject, JsonValue } from "@elgato/utils";
import { type Direction, type Target, Trovul, targetKey } from "./trovul";

const trovul = new Trovul();

const DIRECTION_TITLES: Record<Direction, string> = { receive: "Hear", send: "Send", off: "Off" };
const NEXT: Record<Direction, Direction> = { receive: "send", send: "off", off: "receive" };

type SceneSettings = { sceneId?: string } & JsonObject;
type DirectionSettings = { direction?: Direction | "cycle" } & JsonObject;
type TargetSettings = { target?: Target["target"]; name?: string; remote?: boolean } & JsonObject;

function targetOf(settings: TargetSettings): Target | null {
  const target = settings.target ?? "master";
  if (target === "app" && !settings.name?.trim()) return null;
  return { target, name: settings.name?.trim() || undefined, remote: !!settings.remote };
}

function targetLabel(t: Target): string {
  const what = t.target === "master" ? "Master" : t.target === "incoming" ? "Other PC" : (t.name ?? "App");
  return t.remote ? `${what}\n(there)` : what;
}

/** Tell the property inspector what it needs (scenes, connection). */
async function informInspector(): Promise<void> {
  await streamDeck.ui.sendToPropertyInspector({
    connected: trovul.connected,
    problem: trovul.problem,
    peer: trovul.state?.peer_hostname ?? null,
    scenes: trovul.state?.scenes ?? [],
  } as JsonValue);
}

// ---------- Scene ----------

class SceneAction extends SingletonAction<SceneSettings> {
  override readonly manifestId = "com.trovul.streamdeck.scene";

  override async onWillAppear(ev: WillAppearEvent<SceneSettings>): Promise<void> {
    await this.show(ev.action, ev.payload.settings);
  }

  override async onDidReceiveSettings(ev: DidReceiveSettingsEvent<SceneSettings>): Promise<void> {
    await this.show(ev.action, ev.payload.settings);
  }

  override async onKeyDown(ev: KeyDownEvent<SceneSettings>): Promise<void> {
    const id = ev.payload.settings.sceneId;
    if (!id || !trovul.applyScene(id)) await ev.action.showAlert();
    else await ev.action.showOk();
  }

  override async onSendToPlugin(_ev: SendToPluginEvent<JsonValue, SceneSettings>): Promise<void> {
    await informInspector();
  }

  async show(action: Action<SceneSettings>, settings: SceneSettings): Promise<void> {
    const name = trovul.state?.scenes.find((s) => s.id === settings.sceneId)?.name;
    if (action.isKey()) await action.setTitle(name ?? (settings.sceneId ? "?" : "Scene"));
  }

  refresh(): void {
    for (const action of this.actions) {
      void action.getSettings().then((s) => this.show(action, s));
    }
  }
}

// ---------- Direction ----------

class DirectionAction extends SingletonAction<DirectionSettings> {
  override readonly manifestId = "com.trovul.streamdeck.direction";

  override async onWillAppear(ev: WillAppearEvent<DirectionSettings>): Promise<void> {
    await this.show(ev.action, ev.payload.settings);
  }

  override async onDidReceiveSettings(ev: DidReceiveSettingsEvent<DirectionSettings>): Promise<void> {
    await this.show(ev.action, ev.payload.settings);
  }

  override async onKeyDown(ev: KeyDownEvent<DirectionSettings>): Promise<void> {
    const wanted = (ev.payload.settings.direction ?? "cycle") as Direction | "cycle";
    const current = trovul.state?.direction ?? "off";
    const next = wanted === "cycle" ? NEXT[current] : wanted === current ? "off" : wanted;
    if (!trovul.state?.connected || !trovul.setDirection(next)) await ev.action.showAlert();
  }

  async show(action: Action<DirectionSettings>, settings: DirectionSettings): Promise<void> {
    if (!action.isKey()) return;
    const current = trovul.state?.direction ?? "off";
    const wanted = (settings.direction ?? "cycle") as Direction | "cycle";
    const active = wanted === "cycle" ? current !== "off" : current === wanted;
    await action.setState(active ? 1 : 0);
    await action.setTitle(DIRECTION_TITLES[wanted === "cycle" ? current : wanted]);
  }

  refresh(): void {
    for (const action of this.actions) {
      void action.getSettings().then((s) => this.show(action, s));
    }
  }
}

// ---------- Mute (key) ----------

class MuteAction extends SingletonAction<TargetSettings> {
  override readonly manifestId = "com.trovul.streamdeck.mute";

  override async onWillAppear(ev: WillAppearEvent<TargetSettings>): Promise<void> {
    await this.show(ev.action, ev.payload.settings);
  }

  override async onDidReceiveSettings(ev: DidReceiveSettingsEvent<TargetSettings>): Promise<void> {
    await this.show(ev.action, ev.payload.settings);
  }

  override async onKeyDown(ev: KeyDownEvent<TargetSettings>): Promise<void> {
    const target = targetOf(ev.payload.settings);
    if (!target || !trovul.toggleMute(target)) await ev.action.showAlert();
  }

  override async onSendToPlugin(_ev: SendToPluginEvent<JsonValue, TargetSettings>): Promise<void> {
    await informInspector();
  }

  async show(action: Action<TargetSettings>, settings: TargetSettings): Promise<void> {
    if (!action.isKey()) return;
    const target = targetOf(settings);
    if (!target) {
      await action.setTitle("Choose\nan app");
      return;
    }
    const known = trovul.targets.get(targetKey(target));
    if (!known) trovul.refresh(target);
    await action.setState(known?.muted ? 1 : 0);
    await action.setTitle(targetLabel(target));
  }

  refresh(): void {
    for (const action of this.actions) {
      void action.getSettings().then((s) => this.show(action, s));
    }
  }
}

// ---------- Volume (dial, Stream Deck +) ----------

const STEP = 0.02;

class VolumeAction extends SingletonAction<TargetSettings> {
  override readonly manifestId = "com.trovul.streamdeck.volume";

  override async onWillAppear(ev: WillAppearEvent<TargetSettings>): Promise<void> {
    await this.show(ev.action, ev.payload.settings);
  }

  override async onDidReceiveSettings(ev: DidReceiveSettingsEvent<TargetSettings>): Promise<void> {
    await this.show(ev.action, ev.payload.settings);
  }

  override async onDialRotate(ev: DialRotateEvent<TargetSettings>): Promise<void> {
    const target = targetOf(ev.payload.settings);
    if (!target || !trovul.turn(target, ev.payload.ticks * STEP)) await ev.action.showAlert();
  }

  override async onDialDown(ev: DialDownEvent<TargetSettings>): Promise<void> {
    const target = targetOf(ev.payload.settings);
    if (!target || !trovul.toggleMute(target)) await ev.action.showAlert();
  }

  override async onKeyDown(ev: KeyDownEvent<TargetSettings>): Promise<void> {
    // As a key: mute toggle, like the Mute action.
    const target = targetOf(ev.payload.settings);
    if (!target || !trovul.toggleMute(target)) await ev.action.showAlert();
  }

  override async onSendToPlugin(_ev: SendToPluginEvent<JsonValue, TargetSettings>): Promise<void> {
    await informInspector();
  }

  async show(action: Action<TargetSettings>, settings: TargetSettings): Promise<void> {
    const target = targetOf(settings);
    if (!target) {
      if (action.isDial()) await action.setFeedback({ title: "Choose an app", value: "", indicator: { value: 0 } });
      return;
    }
    const known = trovul.targets.get(targetKey(target));
    if (!known) trovul.refresh(target);
    const percent = Math.round((known?.volume ?? 0) * 100);
    const title = targetLabel(target).replace("\n", " ");
    if (action.isDial()) {
      await action.setFeedback({
        title,
        value: known ? (known.muted ? "Muted" : `${percent}%`) : "…",
        indicator: { value: known?.muted ? 0 : percent },
      });
    } else if (action.isKey()) {
      await action.setTitle(known ? `${title}\n${known.muted ? "Muted" : `${percent}%`}` : title);
    }
  }

  refresh(): void {
    for (const action of this.actions) {
      void action.getSettings().then((s) => this.show(action, s));
    }
  }
}

// ---------- Follow the pointer ----------

class FollowAction extends SingletonAction<JsonObject> {
  override readonly manifestId = "com.trovul.streamdeck.follow";

  override async onWillAppear(ev: WillAppearEvent<JsonObject>): Promise<void> {
    await this.show(ev.action as KeyAction<JsonObject>);
  }

  override async onKeyDown(ev: KeyDownEvent<JsonObject>): Promise<void> {
    if (!trovul.setFollowPointer(!trovul.state?.follow_pointer)) await ev.action.showAlert();
  }

  async show(action: Action<JsonObject>): Promise<void> {
    if (action.isKey()) await action.setState(trovul.state?.follow_pointer ? 1 : 0);
  }

  refresh(): void {
    for (const action of this.actions) void this.show(action);
  }
}

// ---------- Wiring ----------

const scene = new SceneAction();
const direction = new DirectionAction();
const mute = new MuteAction();
const volume = new VolumeAction();
const follow = new FollowAction();
for (const action of [scene, direction, mute, volume, follow]) {
  streamDeck.actions.registerAction(action);
}

trovul.on("state", () => {
  scene.refresh();
  direction.refresh();
  follow.refresh();
  // Mute / volume states may have changed with a scene or the other PC.
  for (const t of trovul.targets.values()) trovul.refresh(t);
});
trovul.on("target", () => {
  mute.refresh();
  volume.refresh();
});
trovul.on("status", () => void informInspector().catch(() => {}));
trovul.on("error-message", (message: string) => streamDeck.logger.warn(`Trovul: ${message}`));

trovul.start();
streamDeck.connect();
