// Things that float over the pages: the pairing prompt, toasts, the splash.

import { useEffect, useRef, useState } from "react";
import { CircleCheck, Info, Monitor, TriangleAlert, X } from "lucide-react";
import { useApp } from "../app/AppContext";
import { Button } from "../ui";
import { Logo } from "./Logo";

/** The backend gives up on an unanswered request after 45 s (connection.rs). */
const REQUEST_TIMEOUT_S = 45;

export function ConnectionRequestDialog() {
  const { incomingRequest: request, answerRequest, appInfo } = useApp();
  const [left, setLeft] = useState(REQUEST_TIMEOUT_S);
  // Focus goes to the dialog itself, never to Accept: the window jumps to the
  // front on its own, and an Enter meant for another app must not pair a PC.
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!request) return;
    setLeft(REQUEST_TIMEOUT_S);
    dialogRef.current?.focus();
    const started = Date.now();
    const timer = window.setInterval(() => {
      setLeft(Math.max(0, REQUEST_TIMEOUT_S - Math.floor((Date.now() - started) / 1000)));
    }, 500);
    return () => window.clearInterval(timer);
  }, [request]);

  if (!request) return null;
  return (
    <div className="dialog-backdrop">
      <div
        ref={dialogRef}
        tabIndex={-1}
        className="dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="request-title"
        aria-describedby="request-text"
      >
        <div className="pair-visual" aria-hidden="true">
          <span className="avatar lit">
            <Monitor />
          </span>
          <span className="pair-beam" />
          <span className="avatar lit" style={{ ["--accent" as string]: "var(--cyan)" }}>
            <Monitor />
          </span>
        </div>
        <h2 id="request-title" className="dialog-title">
          {request.hostname} wants to link
        </h2>
        <p id="request-text" className="dialog-text">
          <span className="mono">{request.ip}</span> is asking to link with {appInfo?.hostname ?? "this PC"}. Once
          accepted, the two PCs stay paired and reconnect on their own.
        </p>
        <div className="dialog-actions">
          <Button onClick={() => answerRequest(false)}>Decline</Button>
          <Button variant="primary" onClick={() => answerRequest(true)}>
            Accept
          </Button>
        </div>
        <p className="dialog-foot">
          Expires in <span className="countdown">{left}s</span>
        </p>
      </div>
    </div>
  );
}

const TOAST_ICONS = { info: Info, success: CircleCheck, error: TriangleAlert };

export function Toasts() {
  const { toasts, dismissToast } = useApp();
  // Always mounted (it takes no space when empty): screen readers only
  // announce changes to a live region that already exists.
  return (
    <div className="toasts" role="status" aria-live="polite">
      {toasts.map((toast) => {
        const Icon = TOAST_ICONS[toast.tone];
        return (
          <div key={toast.id} className={`toast toast-${toast.tone}`}>
            <Icon aria-hidden="true" />
            <span className="toast-message">{toast.message}</span>
            <button className="toast-close" onClick={() => dismissToast(toast.id)} aria-label="Dismiss">
              <X />
            </button>
          </div>
        );
      })}
    </div>
  );
}

/** Brief branded intro while the first backend calls come back. */
export function Splash({ onDone }: { onDone: () => void }) {
  const [leaving, setLeaving] = useState(false);
  const onDoneRef = useRef(onDone);
  onDoneRef.current = onDone;

  useEffect(() => {
    const leave = window.setTimeout(() => setLeaving(true), 850);
    const done = window.setTimeout(() => onDoneRef.current(), 1200);
    return () => {
      window.clearTimeout(leave);
      window.clearTimeout(done);
    };
  }, []);

  return (
    <div className={`splash${leaving ? " leaving" : ""}`} aria-hidden="true">
      <div className="splash-content">
        <Logo size={72} animated />
        <span className="splash-name">Trovul</span>
      </div>
    </div>
  );
}
