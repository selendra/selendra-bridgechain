# Docs

Cross-cutting documentation: how the pieces fit together, and how to run them.

Documentation that describes one directory lives in that directory, not here.

## Read in this order

1. [`../README.md`](../README.md)
   What this is, the repository layout, and how to run the test suites.
2. [`architecture.md`](./architecture.md)
   The system as built, written from the sources with file and line references.
   Covers the `submissionId` invariant and where it is enforced, the three signing prefixes and the domain separation they buy, the trust boundary in `bridge-core/src/store.rs`, the two-phase refund, and which processes are required for which features.
3. [`operations.md`](./operations.md)
   Running the stack, configuring a validator, key custody, the deploy checklist, and incident response.
4. [`../report.md`](../report.md)
   The current list of known defects, with severities and file references.
   `architecture.md` and `operations.md` both cross-reference it by finding id at the points where a defect would bite you.

## Component docs

These stay next to the code they describe, because that is where you will be standing when you need them.

- [`../contracts/README.md`](../contracts/README.md)
- [`../frontend/README.md`](../frontend/README.md)
- [`../crates/solana-gate/README.md`](../crates/solana-gate/README.md)
  Read this one before touching that directory.
  The program is excluded from the workspace and is not safe to deploy as written.

## History

[`history/`](./history/) holds the original build plans.
They are a record of intent, not a description of the system, and they are superseded by `architecture.md` wherever the two disagree.

- [`history/bridge-build-plan.md`](./history/bridge-build-plan.md)
- [`history/swap-build-plan.md`](./history/swap-build-plan.md)
