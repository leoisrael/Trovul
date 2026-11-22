import { useCallback, useState } from "react";
import { useApp } from "../app/AppContext";
import { feature } from "../app/features";
import { inputStatusText, useClipboardView, useInputView } from "../app/useInput";
import { api, errorMessage } from "../api";
import { isolatedScreens, ScreenArrangement } from "../components/ScreenArrangement";
import type { ClipboardView, InputView, Placement } from "../types";
import { ClipboardCopy, Gamepad2, Keyboard, Scan, ShieldCheck } from "lucide-react";
import { Block, Callout, Group, PageHeader, Row, Section, Spinner, Toggle } from "../ui";
import { timeAgo } from "../util";

export function InputPage() {
  const { appInfo, peerName, report } = useApp();
  const [view, setView] = useInputView();
  const [clipboard, setClipboard] = useClipboardView();
  const [busy, setBusy] = useState(false);
  const input = feature("input");
  const peer = view?.peer_name ?? peerName ?? "the other PC";

  const run = useCallback(
    async (action: () => Promise<InputView>) => {
      setBusy(true);
      try {
        setView(await action());
      } catch (e) {
        report(`Keyboard & mouse: ${errorMessage(e)}`);
      } finally {
        setBusy(false);
      }
    },
    [report, setView],
  );

  const status = view ? inputStatusText(view.status, peer) : null;
  const hasRemote = !!view?.screens.some((s) => !s.local);
  const isolated = view && hasRemote ? isolatedScreens(view.screens) : [];

  return (
    <div className="page page-narrow">
      <PageHeader title={input.name} />

      <Section footer="Turn it on on both PCs. Ctrl+Alt+Esc always brings the pointer back.">
        <Group>
          <Row icon={input.icon} iconColor={input.color} title="Share keyboard and mouse" detail={status?.text}>
            {view ? (
              <Toggle
                label="Share keyboard and mouse"
                checked={view.enabled}
                disabled={busy}
                onChange={(enabled) => run(() => api.setInputSharing(enabled))}
              />
            ) : (
              <Spinner />
            )}
          </Row>
          <Row
            icon={ClipboardCopy}
            iconColor={input.color}
            title="Share clipboard"
            detail={clipboardText(clipboard, peer)}
          >
            {clipboard && (
              <Toggle
                label="Share clipboard"
                checked={clipboard.enabled}
                onChange={(enabled) =>
                  api
                    .setClipboardSharing(enabled)
                    .then(setClipboard)
                    .catch((e) => report(`Clipboard: ${errorMessage(e)}`))
                }
              />
            )}
          </Row>
        </Group>
      </Section>

      <Section
        title="Screens"
        action={
          hasRemote && (
            <button className="link-action" disabled={busy} onClick={() => run(api.resetInputLayout)}>
              Reset
            </button>
          )
        }
        footer={
          hasRemote
            ? "Drag the screens to match your desk. The pointer crosses wherever two screens touch."
            : "Connect a PC to place its screens next to yours."
        }
      >
        <Group>
          <Block>
            {view ? (
              <ScreenArrangement
                screens={view.screens}
                thisPcName={appInfo?.hostname ?? "This PC"}
                peerName={hasRemote ? peer : null}
                editable={hasRemote && !busy}
                onCommit={(placements: Placement[]) => run(() => api.setInputLayout(placements))}
              />
            ) : (
              <div className="loading-block">
                <Spinner />
              </div>
            )}
          </Block>
        </Group>
      </Section>

      {isolated.length > 0 && (
        <Callout tone="warning" title="A screen doesn't touch any other">
          The pointer can't reach {isolated.map((s) => s.name).join(", ")}. Drag it until an edge meets another screen.
        </Callout>
      )}
      {hasRemote && view && !view.reachable && isolated.length === 0 && (
        <Callout tone="warning" title={`${peer}'s screens don't touch this PC's`}>
          Drag a screen so its edge meets one of the other PC's, or the pointer has no way across.
        </Callout>
      )}
      {view && (
        <Section title="Behavior" footer="Scroll Lock keeps the pointer on the PC it's on. A drag never goes to the other PC: it carries on to this PC's next screen.">
          <Group>
            <Row icon={Keyboard} iconColor={input.color} title="Keyboard follows the pointer" detail="Typing goes to the window under the pointer when it moves to another screen">
              <Toggle
                label="Keyboard follows the pointer"
                checked={view.focus_follows}
                disabled={busy}
                onChange={(on) => run(() => api.setInputOptions({ focusFollows: on }))}
              />
            </Row>
            <Row icon={Gamepad2} iconColor={input.color} title="Game mode" detail="The pointer stays on this PC while a game is in front, so it never leaves the game">
              <Toggle
                label="Game mode"
                checked={view.game_mode}
                disabled={busy}
                onChange={(on) => run(() => api.setInputOptions({ gameMode: on }))}
              />
            </Row>
            <Row
              icon={ShieldCheck}
              iconColor={input.color}
              title="Admin windows, UAC and lock screen"
              detail={adminText(view.admin_input)}
              tone={view.admin_input === "failed" || view.admin_input === "not_installed" ? "danger" : undefined}
            >
              <Toggle
                label="Admin windows, UAC and lock screen"
                checked={view.admin_input !== "off"}
                disabled={busy}
                onChange={(on) => run(() => api.setAdminInput(on))}
              />
            </Row>
            <Row icon={Scan} iconColor={input.color} title="Keep corners on this PC" detail="No switching in screen corners, so close buttons and the clock stay easy to hit">
              <Toggle
                label="Keep corners on this PC"
                checked={view.guard_corners}
                disabled={busy}
                onChange={(on) => run(() => api.setInputOptions({ guardCorners: on }))}
              />
            </Row>
          </Group>
        </Section>
      )}
    </div>
  );
}

function clipboardText(view: ClipboardView | null, peer: string): string | undefined {
  if (!view) return undefined;
  if (!view.enabled) return "Off";
  if (!view.last) return "Copy text, images or files on one PC, paste them on the other";
  const what = view.last.kind === "image" ? `Image ${view.last.detail}` : view.last.detail;
  return `${what} ${view.last.sent ? `sent to ${peer}` : `from ${peer}`} · ${timeAgo(view.last.at)}`;
}

function adminText(state: InputView["admin_input"]): string {
  switch (state) {
    case "off":
      return "Lets the other PC's keyboard and mouse work in installers, admin apps, UAC prompts and the lock screen. While it's on, any program on this PC could answer UAC prompts too.";
    case "starting":
      return "Starting the input helper…";
    case "ready":
      return "On: the other PC's keyboard and mouse work everywhere on this PC, UAC prompts and the lock screen included";
    case "not_installed":
      return "Needs Trovul installed with the .msi installer";
    case "failed":
      return "Windows didn't let Trovul start its input helper. Reinstall Trovul with the .msi installer.";
  }
}
