import { forwardRef } from "react";
import { cn } from "../../lib/cn";

const fieldClass =
  "h-9 w-full rounded-lg border border-line bg-surface-2 px-3 text-[13px] text-fg placeholder:text-faint transition-colors focus-visible:border-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50";

export const Input = forwardRef<
  HTMLInputElement,
  React.InputHTMLAttributes<HTMLInputElement>
>(({ className, ...props }, ref) => (
  <input ref={ref} className={cn(fieldClass, className)} {...props} />
));
Input.displayName = "Input";

export const Select = forwardRef<
  HTMLSelectElement,
  React.SelectHTMLAttributes<HTMLSelectElement>
>(({ className, ...props }, ref) => (
  <select ref={ref} className={cn(fieldClass, "cursor-pointer", className)} {...props} />
));
Select.displayName = "Select";

/** Uppercase field caption used above inputs/selects. */
export function FieldLabel({
  className,
  ...props
}: React.LabelHTMLAttributes<HTMLLabelElement>) {
  return (
    <label
      className={cn(
        "flex flex-col gap-1.5 text-[11px] font-medium uppercase tracking-wide text-muted",
        className,
      )}
      {...props}
    />
  );
}
