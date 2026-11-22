// The mixer: where the sound between the PCs goes, and every app on both PCs
// with its volume, mute and output device — one deck per PC.

import { useMemo } from "react";
import { Bell, Crosshair } from "lucide-react";
import { useApp } from "../../app/AppContext";
import { api } from "../../api";
import { directionOptions, isCallMode } from "../../components/Audio";
import { useAudioStats } from "../../hooks";
import { Button, Callout, Group, PageHeader, Row, Segmented } from "../../ui";
import { Deck, useMixer, useOutputs, type DeckActions } from "./Deck";
import { Quality } from "./Quality";
import { SceneChips } from "./Scenes";

export function MixerPage() {
  const {
    link,
    connection,
    visible,
    appInfo,
    navigate,
    peerName,
    direction,
    setDirection,
    changingDirection,
    updateSettings,
    testSound,
    notify,
    report,
    settings,
  } = useApp();
  const linked = link === "linked";
  const peer = peerName ?? "the other PC";
  const thisPc = appInfo?.hostname ?? "This PC";

  const local = useMixer(api.localMixer, api.localIcons, visible);
  const remote = useMixer(api.remoteMixer, api.remoteIcons, visible && linked, connection.peer_ip);
  const localOutputs = useOutputs(api.outputDevices, visible);
  const remoteOutputs = useOutputs(api.remoteOutputDevices, visible && linked, connection.peer_ip);
  const stats = useAudioStats(visible, linked && direction !== "off");

  const localActions = useMemo<DeckActions>(
    () => ({
      setMasterVolume: api.setMasterVolume,
      setMasterMute: api.setMasterMute,
      setAppVolume: api.setSessionVolume,
      setAppMute: api.setSessionMute,
      setAppOutput: api.setAppOutput,
      setStreamVolume: async (volume) => updateSettings({ remote_volume: volume }),
      setStreamMute: async (muted) => updateSettings({ remote_muted: muted }),
      setStreamOutput: api.setPlaybackDevice,
      setSendMode: api.setSendMode,
      setAppSend: api.setAppSend,
    }),
    [updateSettings],
  );

  const stream = local.mixer?.stream;
  // Follow the pointer, as the local stream strip reports it (this PC's view).
  const focus = stream?.focus;
  const playing = stream?.receiving ? (localOutputs.find((d) => d.id === stream.output) ?? localOutputs.find((d) => d.is_default)) : undefined;

  return (
    <div className="page page-mixer">
      <PageHeader
        title="Mixer"
        actions={
          linked && (
            <div className="mixer-actions">
              {direction === "receive" && (
                <button
                  type="button"
                  className={`follow-pill${settings?.follow_pointer ? " on" : ""}`}
                  aria-pressed={!!settings?.follow_pointer}
                  title="In the headset, the PC you're using is louder (needs keyboard & mouse sharing on both PCs)"
                  onClick={() => updateSettings({ follow_pointer: !settings?.follow_pointer })}
                >
                  <Crosshair aria-hidden="true" />
                  Follow pointer
                </button>
              )}
              <Segmented
              label="Where the sound between the PCs plays"
              value={direction}
              disabled={changingDirection}
              options={directionOptions(peer)}
              onChange={setDirection}
              />
            </div>
          )
        }
      />
      <SceneChips linked={linked} editable onMessage={(m) => notify(m, "success")} onError={report} />
      <Deck
        name={thisPc}
        tag={
          <>
            <span className="deck-tag">This PC</span>
            <FocusTag focus={focus} />
          </>
        }
        action={
          linked &&
          stream && (
            <Button size="sm" icon={Bell} onClick={testSound} title="Play a short chime through the stream">
              Test sound
            </Button>
          )
        }
        other={peer}
        linked={linked}
        mixer={local.mixer}
        error={local.error}
        icons={local.icons}
        outputs={localOutputs}
        actions={localActions}
        update={local.update}
        emptyText="Play something on this PC and its apps show up here."
      />
      {linked ? (
        <Deck
          name={peer}
          tag={<FocusTag focus={focus === undefined || focus === null ? focus : !focus} />}
          other={thisPc}
          linked
          mixer={remote.mixer}
          error={remote.error}
          icons={remote.icons}
          outputs={remoteOutputs}
          actions={remoteActions}
          update={remote.update}
          emptyText={`Nothing is playing on ${peer}.`}
        />
      ) : (
        <Group>
          <Row
            title={link === "reconnecting" ? `Waiting for ${peer}` : "No second PC"}
            detail={
              link === "reconnecting"
                ? "Its apps show up here again as soon as it reconnects."
                : "Connect a PC to hear it here and mix its apps from this one."
            }
          >
            {link === "unlinked" && (
              <Button size="sm" onClick={() => navigate({ page: "home" })}>
                Connect
              </Button>
            )}
          </Row>
        </Group>
      )}
      {playing && isCallMode(playing) && (
        <Callout tone="warning" title="This headset is in call mode">
          “{playing.name}” plays phone-quality sound while an app uses its microphone. Use another microphone and pick
          the headset's Headphones or Stereo output for {peer}.
        </Callout>
      )}
      {linked && direction !== "off" && stats && <Quality stats={stats} receiving={direction === "receive"} />}
    </div>
  );
}

const remoteActions: DeckActions = {
  setMasterVolume: api.setRemoteMasterVolume,
  setMasterMute: api.setRemoteMasterMute,
  setAppVolume: api.setRemoteSessionVolume,
  setAppMute: api.setRemoteSessionMute,
  setAppOutput: api.setRemoteAppOutput,
  setStreamVolume: (volume) => api.setRemoteIncoming(volume, null),
  setStreamMute: (muted) => api.setRemoteIncoming(null, muted),
  setStreamOutput: api.setRemotePlaybackDevice,
  setSendMode: api.setRemoteSendMode,
  setAppSend: api.setRemoteAppSend,
};

/** Follow the pointer at work: this deck is the PC in use, or the quieter one. */
function FocusTag({ focus }: { focus: boolean | null | undefined }) {
  if (focus === undefined || focus === null) return null;
  return focus ? (
    <span className="focus-tag in">In focus</span>
  ) : (
    <span className="focus-tag out" title="Follow the pointer lowered it while you use the other PC">
      Quieter
    </span>
  );
}
