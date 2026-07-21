import { useEffect, useMemo, useState } from "react";
import { readDecimals, rpcRequest } from "../wallet/eth";
import type { Chain } from "../api/types";

/** Best-effort token decimals per source chain, resolved from the chain
 *  registry's default `token`. A `Submission` itself carries no decimals —
 *  only an opaque `debridgeId` hash — so this is a heuristic for the common
 *  case of one bridged asset per chain; it can't be exact for a chain that
 *  bridges several differently-decimaled tokens. Falls back to 18 (the prior
 *  silent assumption) for chains without a registered token/rpcUrl, or where
 *  the read fails. Re-resolved only when the registry's (chainId, token,
 *  rpcUrl) triples actually change, not on every registry poll tick. */
export function useChainDecimals(chains: Chain[]): Record<number, number> {
  const [decimals, setDecimals] = useState<Record<number, number>>({});
  const targets = useMemo(
    () => chains.filter((c): c is Chain & { rpcUrl: string; token: string } => !!c.rpcUrl && !!c.token),
    [chains]
  );
  const key = targets.map((c) => `${c.chainId}:${c.token}:${c.rpcUrl}`).join(",");

  useEffect(() => {
    if (!key) return;
    let alive = true;
    Promise.all(
      targets.map(async (c) => {
        try {
          return [c.chainId, await readDecimals(rpcRequest(c.rpcUrl), c.token)] as const;
        } catch {
          return [c.chainId, 18] as const;
        }
      })
    ).then((entries) => {
      if (alive) setDecimals(Object.fromEntries(entries));
    });
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  return decimals;
}
