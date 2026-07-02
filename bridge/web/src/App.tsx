import { useState } from "react";
import { useIsFetching } from "@tanstack/react-query";
import { ArrowLeftRight } from "lucide-react";
import type { Submission } from "./lib/api";
import { cn } from "./lib/cn";
import { POLL_MS } from "./hooks/queries";
import { MonitorView } from "./features/monitor/MonitorView";
import { Lookup } from "./features/monitor/Lookup";
import { BridgeView } from "./features/bridge/BridgeView";

type View = "monitor" | "bridge";

const TABS: { id: View; label: string }[] = [
  { id: "monitor", label: "Monitor" },
  { id: "bridge", label: "Bridge" },
];

export function App() {
  const [view, setView] = useState<View>("monitor");
  const [selected, setSelected] = useState<Submission | null>(null);

  // Any in-flight TanStack query pulses the live dot.
  const fetching = useIsFetching() > 0;

  return (
    <div className="mx-auto max-w-[1280px] px-6 pb-12 pt-5">
      <header className="flex flex-wrap items-center justify-between gap-4 border-b border-line pb-4.5">
        <div className="flex items-center gap-3.5">
          <span className="grid size-12 place-items-center rounded-xl bg-gradient-to-br from-accent to-violet text-2xl text-white shadow-lg shadow-accent/20">
            ⇌
          </span>
          <div>
            <h1 className="text-lg font-semibold tracking-tight">
              Bridge Dashboard
            </h1>
            <p className="mt-0.5 text-[12.5px] text-muted">
              EVM ↔ EVM transfers, signatures &amp; on-chain status
            </p>
          </div>
        </div>

        <div className="flex items-center gap-3.5">
          <nav className="flex gap-0.5 rounded-xl border border-line bg-surface-2 p-1">
            {TABS.map((t) => (
              <button
                key={t.id}
                onClick={() => setView(t.id)}
                className={cn(
                  "rounded-lg px-3.5 py-1.5 text-sm font-medium transition-colors",
                  view === t.id
                    ? "bg-accent text-white shadow-sm shadow-accent/20"
                    : "text-muted hover:text-fg",
                )}
              >
                {t.id === "bridge" && (
                  <ArrowLeftRight className="mr-1.5 inline size-3.5 align-[-2px]" />
                )}
                {t.label}
              </button>
            ))}
          </nav>
          {view === "monitor" && <Lookup onFound={setSelected} />}
          <span
            title={`auto-refresh every ${POLL_MS / 1000}s`}
            className={cn(
              "size-2.5 rounded-full bg-success transition-shadow",
              fetching && "animate-pulse shadow-[0_0_0_4px_rgba(52,211,153,0.25)]",
            )}
          />
        </div>
      </header>

      <div className="mt-5 space-y-5">
        {view === "bridge" ? (
          <BridgeView onSent={() => setView("monitor")} />
        ) : (
          <MonitorView selected={selected} onSelect={setSelected} />
        )}
      </div>
    </div>
  );
}
