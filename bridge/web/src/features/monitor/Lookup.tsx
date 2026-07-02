import { useState } from "react";
import { Search } from "lucide-react";
import { fetchSubmission, GqlError, type Submission } from "../../lib/api";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";

interface Props {
  onFound: (s: Submission) => void;
}

/** Direct by-id lookup — exercises the `submission(submissionId:)` query and its
 * boundary validation (a malformed id returns a clean error, not a crash). */
export function Lookup({ onFound }: Props) {
  const [id, setId] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = id.trim();
    if (!trimmed) return;
    setBusy(true);
    setError(null);
    try {
      const found = await fetchSubmission(trimmed);
      if (found) {
        onFound(found);
        setError(null);
      } else {
        setError("No submission with that ID.");
      }
    } catch (e) {
      setError(e instanceof GqlError ? e.message : "Lookup failed.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="flex flex-wrap items-center gap-2" onSubmit={submit}>
      <div className="relative">
        <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-faint" />
        <Input
          type="text"
          placeholder="Look up submissionId (0x + 64 hex)…"
          value={id}
          spellCheck={false}
          onChange={(e) => setId(e.target.value)}
          className="w-[300px] max-w-[48vw] pl-8"
        />
      </div>
      <Button type="submit" size="sm" disabled={busy || id.trim() === ""}>
        {busy ? "…" : "Find"}
      </Button>
      {error && <span className="text-[12px] text-warning">{error}</span>}
    </form>
  );
}
