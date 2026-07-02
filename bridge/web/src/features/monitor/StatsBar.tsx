import { Activity, FileSignature, Layers, ShieldCheck } from "lucide-react";
import type { Stats } from "../../lib/api";
import { Card } from "../../components/ui/card";

function Stat({
  label,
  value,
  hint,
  Icon,
}: {
  label: string;
  value: string;
  hint?: string;
  Icon: typeof Activity;
}) {
  return (
    <Card className="p-4">
      <div className="flex items-center justify-between">
        <span className="text-[11px] font-medium uppercase tracking-wide text-muted">
          {label}
        </span>
        <Icon className="size-4 text-faint" />
      </div>
      <div className="mt-2 text-[26px] font-semibold tracking-tight tnum">
        {value}
      </div>
      {hint && <div className="text-[11.5px] text-faint">{hint}</div>}
    </Card>
  );
}

/** Top row of aggregate numbers plus a compact list of active routes. */
export function StatsBar({ stats }: { stats: Stats }) {
  return (
    <section className="grid grid-cols-2 gap-3 md:grid-cols-4 xl:grid-cols-[repeat(4,1fr)_1.4fr]">
      <Stat
        label="Submissions"
        value={stats.total.toString()}
        Icon={Layers}
      />
      <Stat
        label="Signed"
        value={stats.signed.toString()}
        hint="≥ 1 signature"
        Icon={FileSignature}
      />
      <Stat
        label="Ready to claim"
        value={stats.threshold == null ? "—" : stats.ready.toString()}
        hint={stats.threshold == null ? "no threshold set" : "meet threshold"}
        Icon={Activity}
      />
      <Stat
        label="Threshold"
        value={stats.threshold == null ? "—" : stats.threshold.toString()}
        hint="signatures required"
        Icon={ShieldCheck}
      />

      <Card className="col-span-2 p-4 md:col-span-4 xl:col-span-1">
        <span className="text-[11px] font-medium uppercase tracking-wide text-muted">
          Routes
        </span>
        {stats.routes.length === 0 ? (
          <div className="mt-2 text-[11.5px] text-faint">none</div>
        ) : (
          <ul className="mt-2 flex max-h-[88px] flex-col gap-1 overflow-auto pr-1">
            {stats.routes.map((r) => (
              <li
                key={`${r.chainIdFrom}-${r.chainIdTo}`}
                className="flex justify-between text-[12.5px]"
              >
                <span className="tnum text-fg">
                  {r.chainIdFrom} → {r.chainIdTo}
                </span>
                <span className="text-muted">{r.count}</span>
              </li>
            ))}
          </ul>
        )}
      </Card>
    </section>
  );
}
