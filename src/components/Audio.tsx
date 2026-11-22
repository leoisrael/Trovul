// Audio building blocks shared by Home and the Mixer.

import { CirclePause, Headphones, Send, Volume2, VolumeX } from "lucide-react";
import { useApp } from "../app/AppContext";
import { useLiveValue } from "../hooks";
import type { AudioDevice, Direction } from "../types";
import { Button, Segmented, Slider, type SegmentOption } from "../ui";

export function directionOptions(peer: string): SegmentOption<Direction>[] {
  return [
    { value: "receive", label: "Hear here", icon: Headphones, title: `Play ${peer}'s audio on this PC` },
    { value: "send", label: `Send to ${peer}`, icon: Send, title: `Play this PC's audio on ${peer}` },
    { value: "off", label: "Off", icon: CirclePause, title: "No audio between the PCs" },
  ];
}

/** Compact direction switch (Overview card). */
export function DirectionSwitch() {
  const { direction, setDirection, changingDirection, peerName } = useApp();
  return (
    <Segmented
      full
      label="Audio direction"
      value={direction}
      disabled={changingDirection}
      options={directionOptions(peerName ?? "the other PC")}
      onChange={setDirection}
    />
  );
}

/** Volume (with the live level inside the track) and mute for the audio received. */
export function IncomingVolume({ level }: { level: number }) {
  const { settings, updateSettings } = useApp();
  const [volume, changeVolume] = useLiveValue(settings?.remote_volume ?? 1, (v) => updateSettings({ remote_volume: v }));
  const muted = settings?.remote_muted ?? false;
  return (
    <div className="field-row">
      <Button
        iconOnly
        icon={muted ? VolumeX : Volume2}
        className={muted ? "is-on" : undefined}
        title={muted ? "Unmute the other PC" : "Mute the other PC"}
        aria-pressed={muted}
        onClick={() => updateSettings({ remote_muted: !muted })}
      />
      <Slider value={volume} level={muted ? 0 : level} muted={muted} label="Incoming audio volume" onChange={changeVolume} />
    </div>
  );
}

// Bluetooth headsets expose a second, "Hands-Free" output for calls: mono,
// phone quality. Windows switches to it when an app opens the headset's mic.
export const isCallMode = (device: AudioDevice) => /hands-?free/i.test(device.name);
