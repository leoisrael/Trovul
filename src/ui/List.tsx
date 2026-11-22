// Grouped lists, the backbone of every page (think System Settings): a small
// section title, a rounded group, and rows of "label → control".

import type { ReactNode } from "react";
import { ChevronRight, type LucideIcon } from "lucide-react";

export function Section({
  title,
  action,
  footer,
  children,
}: {
  title?: ReactNode;
  /** Small link-style action on the right of the title (e.g. "Reset"). */
  action?: ReactNode;
  /** One short line under the group. */
  footer?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="section">
      {(title || action) && (
        <div className="section-head">
          {title && <h2 className="section-title">{title}</h2>}
          {action}
        </div>
      )}
      {children}
      {footer && <p className="section-footer">{footer}</p>}
    </section>
  );
}

export function Group({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={`group${className ? ` ${className}` : ""}`}>{children}</div>;
}

/** A row: icon, title, one line of detail, and a control on the right. */
export function Row({
  icon: Icon,
  iconColor,
  title,
  detail,
  children,
  onClick,
  tone,
}: {
  icon?: LucideIcon;
  iconColor?: string;
  title: ReactNode;
  detail?: ReactNode;
  children?: ReactNode;
  /** Makes the whole row a button with a chevron. */
  onClick?: () => void;
  tone?: "danger";
}) {
  const body = (
    <>
      {Icon && (
        <span className="row-icon" style={iconColor ? { background: iconColor } : undefined} aria-hidden="true">
          <Icon />
        </span>
      )}
      <span className="row-text">
        <span className="row-title">{title}</span>
        {detail && <span className="row-detail">{detail}</span>}
      </span>
      {children && <span className="row-control">{children}</span>}
      {onClick && <ChevronRight className="row-chevron" aria-hidden="true" />}
    </>
  );
  const className = `row${onClick ? " clickable" : ""}${tone ? ` ${tone}` : ""}`;
  return onClick ? (
    <button type="button" className={className} onClick={onClick}>
      {body}
    </button>
  ) : (
    <div className={className}>{body}</div>
  );
}

/** Full-width content inside a group (a segmented control, the arrangement…). */
export function Block({ children }: { children: ReactNode }) {
  return <div className="row-block">{children}</div>;
}
