import { useState } from "react";
import { Navbar, type View } from "./components/Navbar";
import { BridgeView } from "./components/BridgeView";
import { SwapView } from "./components/SwapView";
import { Explorer } from "./components/Explorer";
import { fetchChains, health } from "./api/client";
import { usePoll } from "./api/hooks";
import { useWallet } from "./wallet/useWallet";
import type { Chain, SubmissionFilter } from "./api/types";

export default function App() {
  const [view, setView] = useState<View>("swap");
  const [explorerFilter, setExplorerFilter] = useState<SubmissionFilter | undefined>();

  // One wallet connection, shared by the navbar chip and both views.
  const wallet = useWallet();

  // Shared backend state: reachability + the chain registry.
  const online = usePoll<boolean>(() => health(), [], 5000);
  const chainsQ = usePoll<Chain[]>(() => fetchChains(), [], 15000);
  const chains = chainsQ.data ?? [];
  const connected = online.data === null ? null : online.data && chainsQ.error === null;

  const goExplorer = (filter: SubmissionFilter) => {
    setExplorerFilter(filter);
    setView("explorer");
  };

  const wide = view === "explorer";

  return (
    <div className="app">
      <div className="app__panel">
        <Navbar view={view} onNavigate={setView} connected={connected} wallet={wallet} />
        <main className={`app__main${wide ? " app__main--wide" : ""}`}>
          {view === "bridge" && <BridgeView chains={chains} wallet={wallet} onReview={goExplorer} />}
          {view === "swap" && <SwapView chains={chains} wallet={wallet} />}
          {view === "explorer" && <Explorer chains={chains} initialFilter={explorerFilter} />}
        </main>
      </div>
    </div>
  );
}
