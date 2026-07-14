import { Logo, Gear } from "./icons";
import { WalletButton } from "./WalletButton";
import type { WalletState } from "../wallet/useWallet";

export type View = "bridge" | "swap" | "explorer";

interface NavbarProps {
  view: View;
  onNavigate: (view: View) => void;
  /** Backend reachability, drives the status dot. null = still checking. */
  connected: boolean | null;
  wallet: WalletState;
}

const NAV: { label: string; view: View }[] = [
  { label: "Bridge", view: "bridge" },
  { label: "Swap", view: "swap" },
  { label: "Explorer", view: "explorer" },
];

export function Navbar({ view, onNavigate, connected, wallet }: NavbarProps) {
  const status = connected === null ? "checking" : connected ? "online" : "offline";
  const statusText = connected === null ? "Connecting…" : connected ? "Backend live" : "Backend offline";

  return (
    <header className="nav">
      <div className="nav__left">
        <Logo />
        <span className="nav__divider" />
        <nav className="nav__links">
          {NAV.map((item) => (
            <button
              key={item.label}
              type="button"
              className={`nav__link${view === item.view ? " nav__link--active" : ""}`}
              onClick={() => onNavigate(item.view)}
            >
              {item.label}
            </button>
          ))}
        </nav>
      </div>
      <div className="nav__right">
        <span className={`status status--${status}`} title={statusText}>
          <span className="status__dot" />
          {statusText}
        </span>
        <WalletButton wallet={wallet} />
        <button type="button" className="icon-btn nav__gear" aria-label="Settings">
          <Gear size={20} />
        </button>
      </div>
    </header>
  );
}
