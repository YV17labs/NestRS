# The npm trees are not audited

`docs/` and the NestJS SUTs under `bench/sut/` are third-party tooling, never
deployed to a client. An advisory in them is their vendor's to fix, taken on
the next update; their trees hold only what their build uses.

**Refused:** `npm audit` in `just audit` — a check red for a defect only a
vendor can fix, and CPU on every run.
