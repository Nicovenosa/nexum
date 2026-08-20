# Security Policy

## Supported versions

Nexum is pre-release (release candidates only). Security fixes land on `main`
and are rolled into the next release candidate; the latest tag is the only
supported stream.

## Reporting a vulnerability

Please do **not** open a public issue for security vulnerabilities. Use one
of the private channels:

- **GitHub Security Advisories** — private report via
  <https://github.com/Nicovenosa/nexum/security/advisories/new> (preferred).
- **Private email** — if a GitHub account is not available, contact the
  maintainers at the email address listed in the release notes of the
  affected release candidate.

Include, if possible:

- the affected version and platform (Linux / macOS / Windows)
- a minimal reproduction (command, configuration, session transcript)
- the impact you observed

## What to report

Anything that leads to data loss, credential exposure, unauthorized file
access or command execution by an untrusted third party, including issues in
the ACP transport, the permission/HITL layer, the runtime directory
protections, or the installers.

## Response

Reports are acknowledged within 7 days. A fix is released as a new release
candidate on `main` and backported when a supported release is affected.

## Scope

The repository tree on `main`, the `nexum` / `nexum-acp-host` / `agm` binaries,
and the bundled Python sidecars (stdlib-only). Not in scope: third-party
providers reached by the runtime, and code outside this repository.