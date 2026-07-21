//! End-to-end suite for the seed crate — migrates a throwaway database, runs
//! the seed runner and asserts idempotency. The module tree mirrors `src/`.

mod runner;
