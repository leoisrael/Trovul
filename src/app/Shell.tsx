// Window layout: a quiet sidebar (brand, pages, settings, link status) and the
// current page.

import type { CSSProperties, ReactNode } from "react";
import { House, Settings, type LucideIcon } from "lucide-react";
import { Logo } from "../components/Logo";
import { UpdateBanner } from "../components/UpdateBanner";
import { Dot, Spinner } from "../ui";
import { useApp } from "./AppContext";
import { visibleFeatures } from "./features";
import type { Page } from "./router";

export function Shell({ children }: { children: ReactNode }) {
  return (
    <div className="shell">
      <Sidebar />
      <main className="main">
        <UpdateBanner />
        {children}
      </main>
    </div>
  );
}

function Sidebar() {
  const { route, navigate } = useApp();
  return (
    <aside className="sidebar">
      <div className="brand">
        <Logo size={24} />
        <span className="brand-name">Trovul</span>
      </div>

      <nav className="nav" aria-label="Main">
        <NavItem icon={House} label="Home" page="home" current={route.page} onSelect={navigate} />
        {visibleFeatures().map((f) => (
          <NavItem
            key={f.id}
            icon={f.icon}
            label={f.name}
            page={f.id}
            current={route.page}
            accent={f.color}
            soon={f.status === "soon"}
            onSelect={navigate}
          />
        ))}
      </nav>

      <div className="sidebar-footer">
        <NavItem icon={Settings} label="Settings" page="settings" current={route.page} onSelect={navigate} />
        <LinkStatus />
      </div>
    </aside>
  );
}

function NavItem({
  icon: Icon,
  label,
  page,
  current,
  accent,
  soon,
  onSelect,
}: {
  icon: LucideIcon;
  label: string;
  page: Page;
  current: Page;
  accent?: string;
  soon?: boolean;
  onSelect: (route: { page: Page }) => void;
}) {
  const active = current === page;
  return (
    <button
      className={`nav-item${active ? " active" : ""}${soon ? " is-soon" : ""}`}
      style={accent ? ({ "--accent": accent } as CSSProperties) : undefined}
      aria-current={active ? "page" : undefined}
      title={label}
      onClick={() => onSelect({ page })}
    >
      <Icon className="nav-icon" aria-hidden="true" />
      <span className="nav-label">{label}</span>
      {soon && <span className="nav-soon">Soon</span>}
    </button>
  );
}

/** One quiet line: who this PC is linked with. Opens Home. */
function LinkStatus() {
  const { link, peerName, navigate } = useApp();
  const text = link === "linked" ? peerName : link === "reconnecting" ? `Reconnecting to ${peerName}` : "Not connected";
  return (
    <button className={`link-status ${link}`} onClick={() => navigate({ page: "home" })} title={text ?? ""}>
      {link === "reconnecting" ? <Spinner /> : <Dot tone={link === "linked" ? "success" : "neutral"} />}
      <span className="link-status-text">{text}</span>
    </button>
  );
}
