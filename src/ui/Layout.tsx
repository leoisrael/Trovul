import type { ReactNode } from "react";

export function PageHeader({
  title,
  subtitle,
  actions,
  children,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  actions?: ReactNode;
  /** Rendered under the title row, e.g. tabs. */
  children?: ReactNode;
}) {
  return (
    <header className="page-header">
      <div className="page-header-row">
        <div className="page-heading">
          <h1 className="page-title">{title}</h1>
          {subtitle && <p className="page-subtitle">{subtitle}</p>}
        </div>
        {actions && <div className="page-actions">{actions}</div>}
      </div>
      {children}
    </header>
  );
}

export function Stat({ label, value, unit, warn }: { label: string; value: ReactNode; unit?: string; warn?: boolean }) {
  return (
    <div className={`stat${warn ? " warn" : ""}`}>
      <span className="stat-label">{label}</span>
      <span className="stat-value">
        {value}
        {unit && <small>{unit}</small>}
      </span>
    </div>
  );
}
