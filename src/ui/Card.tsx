import type { CSSProperties, ReactNode } from "react";
import type { LucideIcon } from "lucide-react";

export function Card({
  accent,
  className,
  children,
  style,
}: {
  /** Colour for the header icon (e.g. a feature colour). */
  accent?: string;
  className?: string;
  children: ReactNode;
  style?: CSSProperties;
}) {
  return (
    <section
      className={`card${className ? ` ${className}` : ""}`}
      style={accent ? ({ "--accent": accent, ...style } as CSSProperties) : style}
    >
      {children}
    </section>
  );
}

export function CardHeader({
  icon: Icon,
  title,
  desc,
  actions,
}: {
  icon?: LucideIcon;
  title: ReactNode;
  desc?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="card-header">
      {Icon && (
        <span className="card-icon" aria-hidden="true">
          <Icon />
        </span>
      )}
      <div className="card-heading">
        <h2 className="card-title">{title}</h2>
        {desc && <p className="card-desc">{desc}</p>}
      </div>
      {actions && <div className="card-actions">{actions}</div>}
    </div>
  );
}
