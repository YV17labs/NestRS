# The npm trees are not audited

`docs/` and the NestJS SUTs under `bench/sut/` are third-party tooling, never
deployed to a client. An advisory in them is their vendor's to fix, taken on
the next update. The docs tree holds only what its build uses; a SUT's is what
`nest new` scaffolds, kept whole — its linter, formatter and test runner
included — so the bench measures NestJS as its CLI ships it, not a tree we
trimmed (2026-10-08, the move to NestJS 12).

**Refused:** `npm audit` in `just audit` — a check red for a defect only a
vendor can fix, and CPU on every run.
