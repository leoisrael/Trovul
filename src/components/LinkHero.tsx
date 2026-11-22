// The two PCs and the link between them — the Home page's header. The link
// animates in the audio's direction, scaled by its level; one line below says
// what's going on.

import type { CSSProperties } from "react";
import { Monitor } from "lucide-react";
import { useApp } from "../app/AppContext";
import type { AudioStats } from "../types";
import { levelPercent, Spinner } from "../ui";

export function LinkHero({ stats }: { stats: AudioStats | null }) {
  const { appInfo, connection, link, peerName, direction, disconnect } = useApp();
  const linked = link === "linked";
  const flow = linked ? direction : "off";
  const peer = peerName ?? "Other PC";
  const level = stats?.level ?? 0;
  const delay = flow === "receive" && stats && stats.kbps > 0 ? ` · ${Math.round(stats.buffer_ms)} ms` : "";

  const status =
    link === "unlinked"
      ? "Open Trovul on your other PC to connect"
      : link === "reconnecting"
        ? `Reconnecting to ${peer}…`
        : flow === "receive"
          ? `Playing ${peer}'s sound here${delay}`
          : flow === "send"
            ? `Playing this PC's sound on ${peer}`
            : `Connected to ${peer}`;

  return (
    <section className={`card hero hero-${link}`}>
      <div className="hero-grid">
        <DeviceTile name={appInfo?.hostname ?? "This PC"} meta="This PC" source={flow === "send"} accent="var(--violet)" />
        <div
          className={`hero-link link-${link} flow-${flow}`}
          style={
            {
              "--level": levelPercent(flow === "off" ? 0 : level),
              "--level-num": flow === "off" ? 0 : Math.min(1, Math.sqrt(level)),
            } as CSSProperties
          }
          aria-hidden="true"
        >
          <div className="hero-lane">
            {Array.from({ length: 9 }, (_, i) => (
              <span key={i} className="hero-particle" style={{ animationDelay: `${i * 0.18}s` }} />
            ))}
          </div>
        </div>
        <DeviceTile
          name={link === "unlinked" ? "Other PC" : peer}
          meta={linked ? (connection.peer_ip ?? "") : link === "reconnecting" ? "Offline" : "Not connected"}
          source={flow === "receive"}
          accent="var(--cyan)"
          empty={link === "unlinked"}
          waiting={link === "reconnecting"}
        />
      </div>
      <p className="hero-status" role="status">
        {link === "reconnecting" && <Spinner />}
        {status}
        {linked && (
          <>
            <span className="hero-status-sep" aria-hidden="true">
              ·
            </span>
            <button className="link-action" onClick={disconnect}>
              Disconnect
            </button>
          </>
        )}
      </p>
    </section>
  );
}

function DeviceTile({
  name,
  meta,
  source,
  accent,
  empty,
  waiting,
}: {
  name: string;
  meta: string;
  /** The sound comes from this PC. */
  source: boolean;
  accent: string;
  empty?: boolean;
  waiting?: boolean;
}) {
  return (
    <div
      className={`hero-device${source ? " source" : ""}${empty ? " empty" : ""}${waiting ? " waiting" : ""}`}
      style={{ "--accent": accent } as CSSProperties}
    >
      <div className="hero-screen">
        <Monitor aria-hidden="true" />
        {source && (
          <span className="eq" aria-hidden="true">
            <i />
            <i />
            <i />
            <i />
          </span>
        )}
      </div>
      <span className="hero-device-name" title={name}>
        {name}
      </span>
      <span className="hero-device-meta">{meta}</span>
    </div>
  );
}
