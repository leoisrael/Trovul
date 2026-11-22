// Connection quality of the stream, in one line; the numbers on request.

import { useState } from "react";
import { useApp } from "../../app/AppContext";
import type { AudioStats } from "../../types";
import { Block, Button, Dot, Group, Row, Section, Stat } from "../../ui";

function quality(stats: AudioStats): { label: string; tone: "success" | "accent" | "warning" } {
  const total = stats.packets_received + stats.packets_lost;
  const loss = total > 0 ? stats.packets_lost / total : 0;
  if (stats.state === "retrying") return { label: "Recovering", tone: "warning" };
  if (loss > 0.02 || stats.underruns > 10) return { label: "Unstable", tone: "warning" };
  if (loss > 0.005 || stats.underruns > 2) return { label: "Good", tone: "accent" };
  return { label: "Excellent", tone: "success" };
}

export function Quality({ stats, receiving }: { stats: AudioStats; receiving: boolean }) {
  const { settings, updateSettings } = useApp();
  const [open, setOpen] = useState(false);
  const total = stats.packets_received + stats.packets_lost;
  const loss = total > 0 ? (100 * stats.packets_lost) / total : 0;
  const mbps = (stats.kbps / 1000).toFixed(2);
  const q = receiving ? quality(stats) : null;

  return (
    <Section title="Connection">
      <Group>
        <Row
          title={
            <>
              <Dot tone={q?.tone === "warning" ? "warning" : "success"} />
              {q ? q.label : "Sending"}
            </>
          }
          detail={receiving ? `${Math.round(stats.buffer_ms)} ms delay · ${loss.toFixed(2)}% loss · ${mbps} Mbit/s` : `${mbps} Mbit/s`}
        >
          <button className="link-action" onClick={() => setOpen(!open)} aria-expanded={open}>
            {open ? "Hide details" : "Details"}
          </button>
        </Row>
        {q?.tone === "warning" && settings?.latency_mode !== "stable" && (
          <Row title="Sound drops on this network" detail="Stable latency buffers more and drops less.">
            <Button size="sm" variant="primary" onClick={() => updateSettings({ latency_mode: "stable" })}>
              Use Stable
            </Button>
          </Row>
        )}
        {stats.last_error && stats.state === "retrying" && <Row title="Error" detail={stats.last_error} tone="danger" />}
        {open && (
          <Block>
            <div className="stats">
              {receiving ? (
                <>
                  <Stat label="Playing on" value={stats.device ?? "—"} />
                  <Stat label="Buffer target" value={Math.round(stats.target_ms)} unit="ms" />
                  <Stat label="Clock drift" value={`${stats.drift_ppm >= 0 ? "+" : ""}${Math.round(stats.drift_ppm)}`} unit="ppm" />
                  <Stat label="Underruns" value={stats.underruns.toLocaleString()} warn={stats.underruns > 0} />
                  <Stat
                    label="Sample rates"
                    value={stats.input_rate ? `${stats.input_rate / 1000} → ${stats.output_rate / 1000}` : "—"}
                    unit={stats.input_rate ? "kHz" : undefined}
                  />
                  <Stat label="Received" value={stats.packets_received.toLocaleString()} />
                </>
              ) : (
                <>
                  <Stat label="Capturing" value={stats.device ?? "—"} />
                  <Stat
                    label="Format"
                    value={stats.capture_rate ? (stats.capture_rate / 1000).toFixed(1) : "—"}
                    unit={stats.capture_rate ? `kHz · ${stats.capture_channels} ch` : undefined}
                  />
                  <Stat label="Packets sent" value={stats.packets_sent.toLocaleString()} />
                  <Stat label="Send errors" value={stats.send_errors.toLocaleString()} warn={stats.send_errors > 0} />
                </>
              )}
            </div>
          </Block>
        )}
      </Group>
    </Section>
  );
}
