import { RouteIcon, Glyph } from "./icons";
import { tokenBySymbol } from "../data/assets";

export interface RouteHop {
  /** Bridge/protocol name shown on the connector, e.g. "Wormhole". */
  via: string;
  /** Token symbol at the node this hop lands on. */
  token: string;
}

interface RouteBoxProps {
  fromToken: string;
  hops: RouteHop[];
}

/** Node glyph + symbol caption. */
function Node({ token }: { token: string }) {
  const t = tokenBySymbol(token);
  return (
    <div className="route__node">
      <Glyph gradient={t.gradient} size={26} />
      <span className="route__node-label">{token}</span>
    </div>
  );
}

export function RouteBox({ fromToken, hops }: RouteBoxProps) {
  return (
    <div className="route">
      <div className="route__head">
        <span className="route__title">
          <RouteIcon size={17} /> Route
        </span>
        <button type="button" className="route__change">
          Change
        </button>
      </div>

      <div className="route__path">
        <Node token={fromToken} />
        {hops.map((hop, i) => (
          <div className="route__segment" key={i}>
            <span className="route__via">
              <span className="route__via-pill">
                <span className="route__via-dot" />
                {hop.via}
              </span>
            </span>
            <Node token={hop.token} />
          </div>
        ))}
      </div>
    </div>
  );
}
