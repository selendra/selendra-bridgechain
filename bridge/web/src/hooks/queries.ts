// TanStack Query hooks over the GraphQL client. Polling is expressed as
// `refetchInterval`; in-flight requests are cancelled via the AbortSignal the
// query function receives. This replaces the hand-rolled usePoll.
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import {
  fetchStats,
  fetchSubmissions,
  type Stats,
  type Submission,
  type SubmissionFilter,
} from "../lib/api";

/** Auto-refresh cadence for the live dashboard queries. */
export const POLL_MS = 4000;

export function useStats() {
  return useQuery<Stats>({
    queryKey: ["stats"],
    queryFn: ({ signal }) => fetchStats(signal),
    refetchInterval: POLL_MS,
  });
}

export function useSubmissions(filter: SubmissionFilter | undefined) {
  // TanStack hashes the key deterministically, so the object can go straight in.
  return useQuery<Submission[]>({
    queryKey: ["submissions", filter ?? null],
    queryFn: ({ signal }) => fetchSubmissions(filter, signal),
    refetchInterval: POLL_MS,
    // Keep the previous rows visible while a changed filter refetches.
    placeholderData: keepPreviousData,
  });
}
