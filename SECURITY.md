# Security Policy

Security is a first-class concern in NestRS, and reports are handled as such.

## Reporting a vulnerability

**Please do not open a public issue for a security vulnerability.**

Report it privately through GitHub:

1. Go to the [**Security** tab](https://github.com/YV17labs/NestRS/security) of the
   repository.
2. Click **Report a vulnerability** to open a private advisory.

This keeps the report confidential between you and the maintainers until a fix
is ready — no email address is involved, and the discussion stays private even
though the repository is public.

If you can, include:

- the affected crate(s) and version / commit,
- a minimal reproduction or proof of concept,
- the impact you foresee.

## What to expect

Reports are triaged on receipt. You can expect an acknowledgement, a disclosure
timeline agreed with you, a fix shipped in the next patch of the latest `1.x`,
and credit in the advisory — unless you prefer to stay anonymous.

## Supported versions

Every `nest-rs-*` crate versions in **lockstep** (one number across the
workspace), and security fixes target the **latest `1.x`** release:

| Version      | Supported                      |
| ------------ | ------------------------------ |
| latest `1.x` | ✅                              |
| older `1.x`  | ⚠️ upgrade to the latest patch |

## Advisories

Fixed vulnerabilities are published as **GitHub Security Advisories (GHSA)** on
this repository and cross-filed to the [**RustSec advisory database**], so
`cargo audit` / `cargo deny` surface them for every downstream automatically.

[**RustSec advisory database**]: https://rustsec.org/
