import { AlertTriangle, CheckCircle2, Info } from "lucide-react";
import { cva } from "class-variance-authority";
import { cn } from "../../lib/cn";

export type BannerTone = "error" | "success" | "info";

const banner = cva("flex gap-3 rounded-xl border p-4 text-[13px]", {
  variants: {
    tone: {
      error: "border-danger/35 bg-danger/10 text-danger",
      success: "border-success/35 bg-success/10 text-success",
      info: "border-accent/35 bg-accent/10 text-accent",
    } satisfies Record<BannerTone, string>,
  },
  defaultVariants: { tone: "info" },
});

const ICON: Record<BannerTone, typeof Info> = {
  error: AlertTriangle,
  success: CheckCircle2,
  info: Info,
};

export interface BannerProps extends React.HTMLAttributes<HTMLDivElement> {
  tone: BannerTone;
}

export function Banner({ className, tone, children, ...props }: BannerProps) {
  const Icon = ICON[tone];
  return (
    <div className={cn(banner({ tone }), className)} {...props}>
      <Icon className="mt-0.5 size-4 shrink-0" />
      <div className="min-w-0 flex-1 text-fg/90">{children}</div>
    </div>
  );
}
