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

| Step | Within |
| --- | --- |
| Acknowledgement | 3 working days |
| Triage, with a severity and a plan | 14 days |
| Fix released and advisory published | 90 days of the report, 7 when the flaw is exploited |

A fix ships as a patch on every supported line. Its advisory is a GitHub
Security Advisory with a CVE, cross-filed to the [**RustSec advisory database**]
the same day, and the CHANGELOG names both identifiers and credits you — unless
you prefer to stay anonymous. A release is yanked only for a critical flaw, and
only once its fix is published.

## Supported versions

Every `nest-rs-*` crate versions in **lockstep** (one number across the
workspace). A major ships at most every six months, announced four weeks ahead
with its upgrading guide; minors and patches ship when ready.

| Line | Receives |
| --- | --- |
| latest minor of the current major | every fix |
| previous major | security fixes, for three months after the current major's release — the end date is written here when it ships |
| anything older | nothing: upgrade to a supported line |

Never more than two lines are supported at once. [`CHANGELOG.md`](CHANGELOG.md)
records each release; its `[Unreleased]` heading is the work in progress.

## Advisories

Fixed vulnerabilities are published as **GitHub Security Advisories (GHSA)** on
this repository and cross-filed to the [**RustSec advisory database**], so
`cargo audit` / `cargo deny` surface them for every downstream automatically.

[**RustSec advisory database**]: https://rustsec.org/
