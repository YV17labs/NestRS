//! Integration tests mirroring `src/` (see CLAUDE.md). `nest-rs-guards` owns
//! the auth chain, so its guard→response wiring is exercised here in-process
//! (no DB/network): a guard's `check_http` decision must render the right
//! transport response, and a chain must run each guard and short-circuit on a
//! denial.

mod endpoint;
