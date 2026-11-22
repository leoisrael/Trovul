// Home: the link at a glance and the one or two controls people reach for.
// Everything else lives one click away on the feature's page.

import { useApp } from "../app/AppContext";
import { feature } from "../app/features";
import { inputStatusText, useInputView } from "../app/useInput";
import { api, errorMessage } from "../api";
import { DirectionSwitch, IncomingVolume } from "../components/Audio";
import { FeatureTiles } from "../components/Discover";
import { LinkHero } from "../components/LinkHero";
import { FindPc } from "../components/Pcs";
import { useAudioStats } from "../hooks";
import type { AudioStats } from "../types";
import { Badge, Block, Group, Meter, Row, Section, Toggle } from "../ui";

export function HomePage() {
  const { link, direction, visible, settings } = useApp();
  const linked = link === "linked";
  const stats = useAudioStats(visible, linked && direction !== "off");

  return (
    <div className="page page-home">
      <LinkHero stats={stats} />
      {linked ? (
        <>
          <AudioSection stats={stats} />
          <InputSection />
        </>
      ) : (
        <>
          <FindPc />
          {settings && !settings.last_peer && (
            <Section title="Once your PCs are linked">
              <FeatureTiles />
            </Section>
          )}
        </>
      )}
    </div>
  );
}

function More({ page, label = "More" }: { page: "audio" | "input"; label?: string }) {
  const { navigate } = useApp();
  return (
    <button className="link-action" onClick={() => navigate({ page })}>
      {label}
    </button>
  );
}

function AudioSection({ stats }: { stats: AudioStats | null }) {
  const { direction, settings } = useApp();
  const ducked = direction === "receive" && stats?.ducked && !settings?.remote_muted;
  return (
    <Section
      title={
        <>
          Audio
          {ducked && <Badge tone="warning">Lowered for a call</Badge>}
        </>
      }
      action={<More page="audio" label="Mixer" />}
    >
      <Group>
        <Block>
          <DirectionSwitch />
        </Block>
        {direction === "receive" && (
          <Block>
            <IncomingVolume level={stats?.level ?? 0} />
          </Block>
        )}
        {direction === "send" && (
          <Block>
            <Meter level={stats?.level ?? 0} />
          </Block>
        )}
      </Group>
    </Section>
  );
}

function InputSection() {
  const { peerName, report } = useApp();
  const [view, setView] = useInputView();
  const input = feature("input");
  const status = view ? inputStatusText(view.status, view.peer_name ?? peerName ?? "the other PC") : null;
  return (
    <Section title={input.name} action={<More page="input" />}>
      <Group>
        <Row icon={input.icon} iconColor={input.color} title="Share keyboard and mouse" detail={status?.text}>
          {view && (
            <Toggle
              label="Share keyboard and mouse"
              checked={view.enabled}
              onChange={(enabled) =>
                api
                  .setInputSharing(enabled)
                  .then(setView)
                  .catch((e) => report(`Keyboard & mouse: ${errorMessage(e)}`))
              }
            />
          )}
        </Row>
      </Group>
    </Section>
  );
}
