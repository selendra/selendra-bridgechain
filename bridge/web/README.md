# Bridge Dashboard (web)

A React + TypeScript single-page dashboard for the EVM↔EVM bridge. It reads the
`graphql-api` crate (see `../crates/graphql-api`) and shows, with auto-refresh:

- **Stats** — total / signed / ready submissions, the keeper threshold, and per
  source→destination route counts.
- **Submissions table** — filterable by source/destination chain, minimum
  signatures, and readiness; each row shows the route, amount, signature count
  and lifecycle status (`PENDING` / `READY` / `EXECUTED` / `UNKNOWN`).
- **Detail panel** — the full record plus every collected validator signature,
  and on-chain `executed` state when the API was started with a `--gate`.
- **Lookup** — fetch a single submission by its `0x`-prefixed submissionId
  (malformed ids get a clean validation error from the API).

It is **read-mostly**: the dashboard never writes. The `submitSignature` mutation
exists on the API but is a validator/keeper concern, not a UI one.

## Run

1. Start the GraphQL API (read-only is fine):

   ```sh
   # directory-backed
   cargo run -p graphql-api -- --dir sig-store-data --threshold 2
   # …or against the HTTP sig-store, with on-chain status:
   cargo run -p graphql-api -- --store-url http://127.0.0.1:8080 \
     --threshold 2 --gate 1338=http://127.0.0.1:8546,0xYourGate
   ```

   It listens on `127.0.0.1:8088` by default.

2. Start the dashboard:

   ```sh
   cd web
   npm install
   npm run dev      # http://localhost:5173
   ```

   `vite dev` proxies `/graphql` and `/health` to `127.0.0.1:8088`, so there's no
   CORS to configure. Point it elsewhere with `GRAPHQL_API_URL`:

   ```sh
   GRAPHQL_API_URL=http://127.0.0.1:9000 npm run dev
   ```

## Build

```sh
npm run build      # type-checks (tsc -b) then bundles to dist/
npm run preview    # serve the production build
```

For a standalone production bundle that calls the API directly (no dev proxy),
set the absolute endpoint at build time:

```sh
VITE_GRAPHQL_URL=https://bridge.example/graphql npm run build
```

## Layout

```
src/
  lib/api.ts        typed fetch GraphQL client + schema types + queries
  lib/format.ts     hex shortening / wei→ether formatting
  hooks/usePoll.ts  interval polling with request cancellation
  components/       StatsBar, Filters, SubmissionsTable, SubmissionDetail,
                    StatusBadge, Lookup
  App.tsx           wires polling + filters + selection together
```
