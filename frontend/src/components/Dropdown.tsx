import { useEffect, useRef, useState, type ReactNode } from "react";
import { ChevronDown } from "./icons";

export interface DropdownOption {
  value: string;
  label: string;
  glyph: ReactNode;
  sub?: string;
}

interface DropdownProps {
  options: DropdownOption[];
  value: string;
  onChange: (value: string) => void;
  /** Compact = token pill (right side); full = chain selector (left side). */
  variant?: "chain" | "token";
  /** Shown when `value` matches no option (e.g. a hand-typed custom address). */
  placeholder?: string;
}

/** A small custom select — closes on outside click / Escape. */
export function Dropdown({ options, value, onChange, variant = "chain", placeholder = "Custom" }: DropdownProps) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  // Match ONLY on the parent's value. This used to fall back to `options[0]`,
  // which displayed the first token as if it were selected while the parent
  // still held a different address — so the trigger could read "USDC" while the
  // transfer being built was for something else entirely. A control must never
  // claim a selection its owner does not have.
  const selected = options.find((o) => o.value === value) ?? null;

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  // No options yet (e.g. chains still loading from the backend). Hooks above
  // always run, so this early return is safe.
  if (!options.length) {
    return (
      <div className={`dd dd--${variant}`}>
        <button type="button" className="dd__trigger" disabled>
          <span className="dd__label dd__label--empty">—</span>
        </button>
      </div>
    );
  }

  return (
    <div className={`dd dd--${variant}`} ref={ref}>
      <button
        type="button"
        className="dd__trigger"
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        {selected?.glyph}
        <span className={`dd__label${selected ? "" : " dd__label--empty"}`}>
          {selected ? selected.label : placeholder}
        </span>
        <ChevronDown size={16} className="dd__chev" />
      </button>
      {open && (
        <div className="dd__menu" role="listbox">
          {options.map((o) => (
            <button
              key={o.value}
              type="button"
              role="option"
              aria-selected={o.value === value}
              className={`dd__item${o.value === value ? " dd__item--active" : ""}`}
              onClick={() => {
                onChange(o.value);
                setOpen(false);
              }}
            >
              {o.glyph}
              <span className="dd__item-text">
                <span className="dd__item-label">{o.label}</span>
                {o.sub && <span className="dd__item-sub">{o.sub}</span>}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
