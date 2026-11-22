// Invitation to take a newer Trovul from the linked PC (src-tauri/src/update.rs).

import { useState } from "react";
import { Download, RefreshCw, Sparkles, TriangleAlert } from "lucide-react";
import { useApp } from "../app/AppContext";
import { api, errorMessage } from "../api";
import { Button } from "../ui";

export function UpdateBanner() {
  const { update, report } = useApp();
  const [starting, setStarting] = useState(false);
  if (!update?.available && update?.stage !== "installing") return null;
  const offer = update.available;
  const megabytes = offer ? (offer.size / 1_048_576).toFixed(1) : "";

  const install = () => {
    setStarting(true);
    api
      .installUpdate()
      .catch((e) => report(`Update: ${errorMessage(e)}`))
      .finally(() => setStarting(false));
  };

  return (
    <div className="update-slot">
      <div className={`update-banner stage-${update.stage}`} role="status">
        <span className="update-icon" aria-hidden="true">
          {update.stage === "failed" ? <TriangleAlert /> : update.stage === "idle" ? <Sparkles /> : <RefreshCw className="spin" />}
        </span>
        <div className="update-text">
          {update.stage === "idle" && offer && (
            <>
              <strong>Trovul {offer.version} is on {offer.from}</strong>
              <span>Update this PC with one click: the installer comes straight from {offer.from} ({megabytes} MB), no download needed.</span>
            </>
          )}
          {update.stage === "downloading" && offer && (
            <>
              <strong>Getting Trovul {offer.version} from {offer.from}…</strong>
              <div className="update-progress" aria-label="Download progress">
                <div style={{ width: `${Math.round(update.progress * 100)}%` }} />
              </div>
            </>
          )}
          {update.stage === "installing" && (
            <>
              <strong>Installing{offer ? ` Trovul ${offer.version}` : ""}…</strong>
              <span>Windows may ask for permission. Trovul closes and opens again by itself.</span>
            </>
          )}
          {update.stage === "failed" && (
            <>
              <strong>The update didn't go through</strong>
              <span>{update.error}</span>
            </>
          )}
        </div>
        {(update.stage === "idle" || update.stage === "failed") && offer && (
          <div className="update-actions">
            <Button variant="ghost" size="sm" onClick={() => api.dismissUpdate().catch(() => {})}>
              Later
            </Button>
            <Button variant="primary" size="sm" icon={Download} loading={starting} onClick={install}>
              {update.stage === "failed" ? "Try again" : "Update now"}
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}
