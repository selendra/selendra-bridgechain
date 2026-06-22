import type { Stats } from "../lib/api";

function Card({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="stat-card">
      <div className="stat-value">{value}</div>
      <div className="stat-label">{label}</div>
      {hint && <div className="stat-hint">{hint}</div>}
    </div>
  );
}

/** Top row of aggregate numbers plus a compact list of active routes. */
export function StatsBar({ stats }: { stats: Stats }) {
  return (
    <section className="stats-bar">
      <Card label="Submissions" value={stats.total.toString()} />
      <Card label="Signed" value={stats.signed.toString()} hint="≥ 1 signature" />
      <Card
        label="Ready to claim"
        value={stats.threshold == null ? "—" : stats.ready.toString()}
        hint={stats.threshold == null ? "no threshold set" : "meet threshold"}
      />
      <Card
        label="Threshold"
        value={stats.threshold == null ? "—" : stats.threshold.toString()}
        hint="signatures required"
      />
      <div className="stat-card stat-routes">
        <div className="stat-label">Routes</div>
        {stats.routes.length === 0 ? (
          <div className="stat-hint">none</div>
        ) : (
          <ul>
            {stats.routes.map((r) => (
              <li key={`${r.chainIdFrom}-${r.chainIdTo}`}>
                <span className="route">
                  {r.chainIdFrom} → {r.chainIdTo}
                </span>
                <span className="route-count">{r.count}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </section>
  );
}
