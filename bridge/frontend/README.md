# Bridge Frontend

The bridge UI. Design is based on `../../bridge.png` (a deBridge-style swap
screen), but it is wired to the repo's real backend — the `graphql-api` service
over the signature store — so the two run together. React 18 + TypeScript +
Vite. No external asset/icon deps (all glyphs + icons are inline SVG).

## Run everything (backend + frontend)

```bash
bash ../scripts/run-dev.sh
```

That launches `graphql-api` on `127.0.0.1:8088` and the Vite dev server on
`127.0.0.1:5173` (detached), then verifies both. Open http://localhost:5173.

### Frontend only

```bash
npm install
npm run dev        # http://localhost:5173, proxies /graphql -> :8088
npm run build      # tsc -b + production bundle in dist/
```

The dev server proxies `/graphql` and `/health` to the backend (see
`vite.config.ts`), so the app talks to it same-origin. For a production build,
serve `dist/` behind the same origin as `graphql-api`, or set `VITE_API` to the
backend base URL.

## How it uses the backend

The backend (`crates/graphql-api`) is a read view over the signature store:
`stats`, `submissions(filter)`, `submission(id)`, `chains`, and a
`submitSignature` mutation. The UI consumes it live:

- **Wallet** — real MetaMask / injected EVM connect (EIP-1193): `Connect Wallet`
  → `eth_requestAccounts`, shows the account + network, reacts to
  account/chain changes, disconnect menu. No wallet detected → links to
  MetaMask download. See `src/wallet/useWallet.ts`.
- **Backend status** — a health-polled indicator in the navbar (green = live).
- **Chain selectors** are populated from the `chains` query (discovered, not
  hardcoded). With the default `chains.json` that's Anvil A/B/C.
- **Swap** shows a live "Transfers on this route" count from `stats.routes` for
  the selected corridor; **Review Transaction** jumps to the Explorer filtered
  to that corridor.
- **Explorer** is fully live (polled every ~5s): stat cards from `stats`, a
  filterable/searchable table from `submissions`, and a detail drawer that
  fetches the full record (signatures included) via `submission(id)`.

The swap **quote** (rate / fee / route hops) is indicative — the backend is a
signature store, not a pricing engine. Everything else is real backend data.

## Layout

```
src/
  main.tsx              app entry
  App.tsx               shell; owns view (swap/explorer) + shared backend polls
  index.css             all styling (design tokens as CSS variables)
  api/
    client.ts           GraphQL client + typed queries (fetchStats/Submissions/…)
    hooks.ts            usePoll — interval fetch, keeps last good data
    types.ts            wire types mirroring the graphql-api schema
  data/assets.ts        chain/token visuals + formatters (formatUnits, shortHex)
  components/
    Navbar.tsx          nav + backend status + wallet chips
    SwapCard.tsx        swap form; live chains + route activity
    AmountRow.tsx       a from/to row (chain + token dropdowns)
    RouteBox.tsx        route path visualization (indicative)
    Explorer.tsx        live submissions table + stats + filters
    SubmissionDetail.tsx  detail drawer (full record via submission(id))
    StatusBadge.tsx     PENDING/READY/EXECUTED pill
    Dropdown.tsx        reusable custom select
    icons.tsx           inline SVG icons + gradient glyphs
```
