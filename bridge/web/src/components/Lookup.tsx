import { useState } from "react";
import { fetchSubmission, GqlError, type Submission } from "../lib/api";

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
    <form className="lookup" onSubmit={submit}>
      <input
        type="text"
        placeholder="Look up submissionId (0x + 64 hex)…"
        value={id}
        spellCheck={false}
        onChange={(e) => setId(e.target.value)}
      />
      <button type="submit" disabled={busy || id.trim() === ""}>
        {busy ? "…" : "Find"}
      </button>
      {error && <span className="lookup-error">{error}</span>}
    </form>
  );
}
