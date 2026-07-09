import { useState } from "react";
import { Navbar, type View } from "./components/Navbar";
import { SwapCard } from "./components/SwapCard";
import { Explorer } from "./components/Explorer";
import { fetchChains, fetchStats, health } from "./api/client";
import { usePoll } from "./api/hooks";
import type { Chain, Stats, SubmissionFilter } from "./api/types";

export default function App() {
  const [view, setView] = useState<View>("swap");
  const [explorerFilter, setExplorerFilter] = useState<SubmissionFilter | undefined>();

  // Shared, app-wide backend state: reachability, the chain registry, and stats.
  const online = usePoll<boolean>(() => health(), [], 5000);
  const chains = usePoll<Chain[]>(() => fetchChains(), [], 15000);
  const stats = usePoll<Stats>(() => fetchStats(), [], 5000);

  const chainList = chains.data ?? [];
  const connected = online.data === null ? null : online.data && chains.error === null;

  const goExplorer = (filter: SubmissionFilter) => {
    setExplorerFilter(filter);
    setView("explorer");
  };

  return (
    <div className="app">
      <div className="app__panel">
        <Navbar view={view} onNavigate={setView} connected={connected} />
        <main className={`app__main${view === "explorer" ? " app__main--wide" : ""}`}>
          {view === "swap" ? (
            <SwapCard chains={chainList} stats={stats.data} onReview={goExplorer} />
          ) : (
            <Explorer chains={chainList} initialFilter={explorerFilter} />
          )}
        </main>
      </div>
    </div>
  );
}
