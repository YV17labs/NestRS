# Security Policy

Security reports are taken seriously at any stage.

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

This is a small, volunteer-run project, so there is no formal SLA — but you can
expect an initial acknowledgement and a good-faith effort to triage, fix, and
credit the report (unless you prefer to stay anonymous).

## Supported versions

Until the `1.0` release, only the latest `main` is supported — fixes land there.

From `1.0` on, every `nest-rs-*` crate versions in **lockstep** (one number
across the workspace), and security fixes target the **latest `1.x`** release:

| Version            | Supported                        |
| ------------------ | -------------------------------- |
| latest `1.x`       | ✅                                |
| older `1.x`        | ⚠️ upgrade to the latest patch   |
| `0.x` pre-releases | ❌                                |

## Advisories

Fixed vulnerabilities are published as **GitHub Security Advisories (GHSA)** on
this repository and cross-filed to the [**RustSec advisory database**], so
`cargo audit` / `cargo deny` surface them for every downstream automatically.

[**RustSec advisory database**]: https://rustsec.org/
