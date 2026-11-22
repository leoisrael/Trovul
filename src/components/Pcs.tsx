// Finding and pairing PCs: nearby list, connect by IP, the Public-network
// warning, the PC name field.

import { useEffect, useState, type FormEvent } from "react";
import { Monitor, WifiOff } from "lucide-react";
import { useApp } from "../app/AppContext";
import { api, errorMessage } from "../api";
import { usePolling } from "../hooks";
import type { NetworkProfile } from "../types";
import { Badge, Block, Button, Callout, Group, Row, Section } from "../ui";

const SEARCH_HELP_AFTER_MS = 20_000;

/** Home, when no PC is linked: nearby PCs, then the less common ways in. */
export function FindPc() {
  const { peers, connectingIp, connect } = useApp();
  const network = useNetworkProfile();
  const [searchStarted] = useState(() => Date.now());
  const [byIp, setByIp] = useState(false);
  const [help, setHelp] = useState(false);
  // Peers are polled every 2 s, which also re-renders this check.
  const searchingLong = peers.length === 0 && Date.now() - searchStarted > SEARCH_HELP_AFTER_MS;

  return (
    <>
      <PublicNetworkWarning network={network} />
      <Section
        title="Nearby PCs"
        footer={
          <span className="footer-links">
            <button className="link-action" onClick={() => setByIp(!byIp)}>
              Connect by IP address
            </button>
            {searchingLong && (
              <button className="link-action" onClick={() => setHelp(!help)}>
                Can't find your PC?
              </button>
            )}
          </span>
        }
      >
        <Group>
          {peers.length === 0 && (
            <Block>
              <div className="searching">
                <span className="searching-dot" aria-hidden="true" />
                Looking for PCs with Trovul on this network…
              </div>
            </Block>
          )}
          {peers.map((peer) => {
            const connecting = connectingIp === peer.ip;
            return (
              <Row
                key={peer.id}
                icon={Monitor}
                iconColor={peer.compatible ? "var(--gradient-action)" : undefined}
                title={
                  <>
                    {peer.hostname}
                    {peer.paired && <Badge tone="success">Paired</Badge>}
                  </>
                }
                detail={<span className="mono">{peer.ip}</span>}
              >
                {peer.compatible ? (
                  <Button
                    variant={peer.paired ? "primary" : "secondary"}
                    size="sm"
                    loading={connecting}
                    disabled={connectingIp !== null}
                    onClick={() => connect(peer.ip, peer.id, peer.hostname)}
                  >
                    {connecting ? (peer.paired ? "Connecting…" : "Waiting for approval…") : "Connect"}
                  </Button>
                ) : (
                  <span className="subtle" title="Install the same Trovul version on both PCs">
                    Needs update
                  </span>
                )}
              </Row>
            );
          })}
          {byIp && (
            <Block>
              <ConnectByIp />
            </Block>
          )}
        </Group>
      </Section>
      {help && (
        <Callout tone="info" title="Can't find your PC?">
          <ul>
            <li>Make sure Trovul is open on it and both PCs use the same network (not a guest Wi-Fi).</li>
            <li>If Windows asked about the firewall, allow Trovul on private networks — on both PCs.</li>
            <li>Some routers block discovery: connect with the other PC's IP address instead.</li>
          </ul>
        </Callout>
      )}
    </>
  );
}

export function ConnectByIp() {
  const { connect, connectingIp, connection } = useApp();
  const [address, setAddress] = useState("");

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const ip = address.trim();
    if (ip) connect(ip);
  };

  return (
    <form className="field-row" onSubmit={submit}>
      <input
        className="input mono"
        value={address}
        onChange={(e) => setAddress(e.target.value)}
        placeholder="IP address, e.g. 192.168.1.20"
        aria-label="IP address of the other PC"
        autoFocus
      />
      <Button type="submit" disabled={!address.trim() || connectingIp !== null || connection.connected}>
        Connect
      </Button>
    </form>
  );
}

/** Polls the Windows network profile; the answer drives a Public-network warning. */
export function useNetworkProfile(): NetworkProfile | null {
  const { visible } = useApp();
  const [network, setNetwork] = useState<NetworkProfile | null>(null);
  // Re-checked so the warning goes away once the user fixes it.
  usePolling(async () => {
    setNetwork(await api.networkProfile());
  }, 5000, visible);
  return network;
}

export function PublicNetworkWarning({ network }: { network: NetworkProfile | null }) {
  const { report } = useApp();
  if (network?.category !== "public") return null;
  return (
    <Callout
      tone="warning"
      icon={WifiOff}
      title="This network is set to Public"
      actions={
        <Button
          size="sm"
          onClick={() => api.openNetworkSettings(network.wifi).catch((e) => report(`Network settings: ${errorMessage(e)}`))}
        >
          Fix it
        </Button>
      }
    >
      Windows hides this PC on public networks. Set “{network.name}” to Private, on both PCs.
    </Callout>
  );
}

/** "Name of this PC": saves on Enter or blur; Escape puts the saved name back. */
export function PcNameField() {
  const { settings, appInfo, updateSettings, notify } = useApp();
  const value = settings?.pc_name ?? null;
  const [draft, setDraft] = useState(value ?? "");
  useEffect(() => setDraft(value ?? ""), [value]);

  const save = () => {
    const name = draft.trim();
    if (name === (value ?? "")) {
      setDraft(name);
      return;
    }
    updateSettings({ pc_name: name });
    notify(
      `${name ? `This PC is now “${name}”.` : "This PC now uses its Windows name."} ` +
        "A PC that's already connected shows the new name after they reconnect.",
      "success",
    );
  };

  return (
    <input
      className="input pc-name-input"
      value={draft}
      maxLength={32}
      placeholder={appInfo?.computer_name ?? ""}
      aria-label="Name of this PC"
      onChange={(e) => setDraft(e.target.value)}
      onBlur={save}
      onKeyDown={(e) => {
        if (e.key === "Enter") e.currentTarget.blur();
        if (e.key === "Escape") setDraft(value ?? "");
      }}
    />
  );
}
