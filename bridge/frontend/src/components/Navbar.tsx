import { Logo, Gear } from "./icons";
import { WalletButton } from "./WalletButton";

export type View = "swap" | "explorer";

interface NavbarProps {
  view: View;
  onNavigate: (view: View) => void;
  /** Backend reachability, drives the status dot. null = still checking. */
  connected: boolean | null;
}

const NAV: { label: string; view?: View; href?: string }[] = [
  { label: "Bridge", view: "swap" },
  { label: "Explorer", view: "explorer" },
];

export function Navbar({ view, onNavigate, connected }: NavbarProps) {
  const status = connected === null ? "checking" : connected ? "online" : "offline";
  const statusText =
    connected === null ? "Connecting…" : connected ? "Backend live" : "Backend offline";

  return (
    <header className="nav">
      <div className="nav__left">
        <Logo />
        <span className="nav__divider" />
        <nav className="nav__links">
          {NAV.map((item) =>
            item.view ? (
              <button
                key={item.label}
                type="button"
                className={`nav__link${view === item.view ? " nav__link--active" : ""}`}
                onClick={() => onNavigate(item.view!)}
              >
                {item.label}
              </button>
            ) : (
              <a key={item.label} href={item.href} className="nav__link">
                {item.label}
              </a>
            )
          )}
        </nav>
      </div>
      <div className="nav__right">
        <span className={`status status--${status}`} title={statusText}>
          <span className="status__dot" />
          {statusText}
        </span>
        <WalletButton />
        <button type="button" className="icon-btn nav__gear" aria-label="Settings">
          <Gear size={20} />
        </button>
      </div>
    </header>
  );
}
