// The connection to Trovul's companion API (src-tauri/src/companion_server.rs).
// Trovul writes companion.json (port, token) into its app-data folder while
// the API is on, so the plugin connects without asking for the token. It keeps
// reconnecting, and keeps the latest state for the keys to show.

import { EventEmitter } from "node:events";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import WebSocket from "ws";

export type Direction = "receive" | "send" | "off";

export interface TrovulState {
  connected: boolean;
  peer_hostname: string | null;
  direction: Direction;
  incoming: { volume: number; muted: boolean };
  follow_pointer: boolean;
  send_apps_only: boolean;
  scenes: { id: string; name: string }[];
}

/** What a key or dial controls. */
export interface Target {
  target: "master" | "incoming" | "app";
  /** The app's name, as Trovul's mixer shows it. */
  name?: string;
  /** On the other PC instead of this one. */
  remote?: boolean;
}

export interface TargetState extends Target {
  volume: number;
  muted: boolean;
}

export const targetKey = (t: Target) => `${t.target}|${(t.name ?? "").toLowerCase()}|${t.remote ? 1 : 0}`;

const RETRY_MS = 3000;

function companionFile(): string {
  return process.env.TROVUL_COMPANION_FILE ?? join(process.env.APPDATA ?? "", "com.trovul.app", "companion.json");
}

export class Trovul extends EventEmitter {
  state: TrovulState | null = null;
  /** Last known volume/mute per target (from `adjust` answers). */
  targets = new Map<string, TargetState>();
  /** Why it isn't connected, for the property inspector. */
  problem: string | null = "Connecting to Trovul…";
  private socket: WebSocket | null = null;
  private timer: NodeJS.Timeout | null = null;
  private stopped = false;

  start(): void {
    this.stopped = false;
    this.connect();
  }

  stop(): void {
    this.stopped = true;
    if (this.timer) clearTimeout(this.timer);
    this.socket?.close();
  }

  get connected(): boolean {
    return this.socket?.readyState === WebSocket.OPEN;
  }

  private retry(problem: string): void {
    if (this.problem !== problem) {
      this.problem = problem;
      this.emit("status");
    }
    if (this.stopped || this.timer) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      this.connect();
    }, RETRY_MS);
  }

  private connect(): void {
    let config: { port: number; token: string };
    try {
      config = JSON.parse(readFileSync(companionFile(), "utf8"));
    } catch {
      this.retry("Turn on “Companion API” in Trovul → Settings");
      return;
    }
    const socket = new WebSocket(`ws://127.0.0.1:${config.port}/?token=${encodeURIComponent(config.token)}`);
    this.socket = socket;
    socket.on("open", () => {
      this.problem = null;
      this.emit("status");
      this.send("get_state");
    });
    socket.on("message", (data) => this.receive(String(data)));
    socket.on("close", () => {
      if (this.socket === socket) this.socket = null;
      this.retry("Trovul isn't running");
    });
    socket.on("error", () => {
      // "close" follows and schedules the retry.
    });
  }

  private receive(text: string): void {
    let message: { type: string; payload: unknown };
    try {
      message = JSON.parse(text);
    } catch {
      return;
    }
    switch (message.type) {
      case "state":
        this.state = message.payload as TrovulState;
        this.emit("state", this.state);
        break;
      case "target": {
        const t = message.payload as TargetState;
        this.targets.set(targetKey(t), t);
        this.emit("target", t);
        break;
      }
      case "scene_applied":
        this.emit("scene", message.payload);
        break;
      case "error":
        this.emit("error-message", (message.payload as { message?: string })?.message ?? "Error");
        break;
    }
  }

  /** Fire and forget; answers arrive as events. False when not connected. */
  send(type: string, payload: object = {}): boolean {
    if (!this.connected) return false;
    this.socket!.send(JSON.stringify({ type, payload }));
    return true;
  }

  applyScene(id: string): boolean {
    return this.send("apply_scene", { id });
  }

  setDirection(direction: Direction): boolean {
    return this.send("set_direction", { direction });
  }

  setFollowPointer(on: boolean): boolean {
    return this.send("set_follow_pointer", { on });
  }

  /** Ask for a target's state without changing it. */
  refresh(target: Target): boolean {
    return this.send("adjust", { ...target });
  }

  toggleMute(target: Target): boolean {
    return this.send("adjust", { ...target, mute: "toggle" });
  }

  turn(target: Target, delta: number): boolean {
    return this.send("adjust", { ...target, delta });
  }
}
