import type { ReactNode } from "react";
import { CircleCheck, Info, TriangleAlert, type LucideIcon } from "lucide-react";

type Tone = "neutral" | "accent" | "success" | "warning" | "danger";

export function Badge({ tone = "neutral", icon: Icon, children, title }: {
  tone?: Tone | "soon";
  icon?: LucideIcon;
  children: ReactNode;
  title?: string;
}) {
  return (
    <span className={`badge${tone === "neutral" ? "" : ` badge-${tone}`}`} title={title}>
      {Icon && <Icon aria-hidden="true" />}
      {children}
    </span>
  );
}

export function Dot({ tone = "neutral", pulse }: { tone?: Tone; pulse?: boolean }) {
  return <span className={`dot${tone === "neutral" ? "" : ` dot-${tone}`}${pulse ? " pulse" : ""}`} aria-hidden="true" />;
}

const CALLOUT_ICONS: Record<string, LucideIcon> = {
  info: Info,
  warning: TriangleAlert,
  danger: TriangleAlert,
  success: CircleCheck,
};

export function Callout({
  tone = "info",
  icon,
  title,
  children,
  actions,
}: {
  tone?: "info" | "warning" | "danger" | "success";
  icon?: LucideIcon;
  title?: ReactNode;
  children?: ReactNode;
  actions?: ReactNode;
}) {
  const Icon = icon ?? CALLOUT_ICONS[tone];
  return (
    <div className={`callout callout-${tone}`} role={tone === "danger" ? "alert" : undefined}>
      <Icon aria-hidden="true" />
      <div className="callout-body">
        {title && <span className="callout-title">{title}</span>}
        {children && <div className="callout-text">{children}</div>}
      </div>
      {actions && <div className="callout-actions">{actions}</div>}
    </div>
  );
}

export function Spinner({ large }: { large?: boolean }) {
  return <span className={`spinner${large ? " lg" : ""}`} aria-hidden="true" />;
}

export function EmptyState({
  icon: Icon,
  title,
  text,
  action,
}: {
  icon: LucideIcon;
  title: ReactNode;
  text?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="empty">
      <span className="empty-icon" aria-hidden="true">
        <Icon />
      </span>
      <span className="empty-title">{title}</span>
      {text && <span className="empty-text">{text}</span>}
      {action}
    </div>
  );
}
