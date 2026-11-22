import type { CSSProperties } from "react";
import { Check, Rocket } from "lucide-react";
import type { Feature } from "../app/features";
import { Badge, Card } from "../ui";

/** Placeholder for a capability of the link that isn't built yet. */
export function ComingSoonPage({ feature: f }: { feature: Feature }) {
  const Icon = f.icon;
  return (
    <div className="page page-narrow" style={{ "--accent": f.color } as CSSProperties}>
      <Card className="soon-card">
        <div className="soon-visual" aria-hidden="true">
          <span className="soon-orbit" />
          <span className="soon-orbit two" />
          <span className="soon-icon">
            <Icon />
          </span>
        </div>
        <Badge tone="soon" icon={Rocket}>
          Coming soon
        </Badge>
        <h1 className="soon-title">{f.name}</h1>
        <p className="soon-tagline">{f.tagline}.</p>
        <ul className="soon-list">
          {f.highlights.map((item) => (
            <li key={item}>
              <Check aria-hidden="true" />
              {item}
            </li>
          ))}
        </ul>
        <p className="hint">It will work over the same link as audio — no extra pairing.</p>
      </Card>
    </div>
  );
}
