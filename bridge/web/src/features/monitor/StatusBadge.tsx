import { CircleCheck, CircleDashed, Clock, HelpCircle } from "lucide-react";
import type { SubmissionStatus } from "../../lib/api";
import { Badge, type BadgeProps } from "../../components/ui/badge";

const META: Record<
  SubmissionStatus,
  { label: string; tone: NonNullable<BadgeProps["tone"]>; Icon: typeof Clock }
> = {
  EXECUTED: { label: "Executed", tone: "success", Icon: CircleCheck },
  READY: { label: "Ready", tone: "accent", Icon: CircleDashed },
  PENDING: { label: "Pending", tone: "warning", Icon: Clock },
  UNKNOWN: { label: "Unknown", tone: "neutral", Icon: HelpCircle },
};

/** Colored pill for a submission's lifecycle status. */
export function StatusBadge({ status }: { status: SubmissionStatus }) {
  const { label, tone, Icon } = META[status];
  return (
    <Badge tone={tone}>
      <Icon className="size-3" />
      {label}
    </Badge>
  );
}
