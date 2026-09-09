# Security Policy

## Supported versions

muri is pre-1.0 and under active development. Security fixes are applied to the
latest released `0.x` line and to `main`. Older `0.x` releases are not
separately patched.

| Version | Supported          |
|---------|--------------------|
| `0.0.x` (latest) | :white_check_mark: |
| older `0.0.x`    | :x:                |

## Reporting a vulnerability

Please report security vulnerabilities **privately** — do not open a public
issue for anything that could be exploited.

Use GitHub's private vulnerability reporting:

1. Go to <https://github.com/MattJackson/muri/security/advisories>.
2. Click **"Report a vulnerability"** to open a private security advisory.

Include as much detail as you can:

- the affected version(s) and platform (macOS / Windows / Linux),
- a description of the issue and its impact,
- reproduction steps or a proof of concept if available.

You can expect an initial acknowledgement within a few days. Once a fix is
ready, a patched release will be published and the advisory disclosed, with
credit to the reporter unless anonymity is requested.

## Scope note

muri is a UI crate (tray icon + styled popup menus). It does not handle
credentials, network I/O, or persistence itself — consuming applications own
that. Reports about how a consuming app uses muri should go to that app's
maintainers.
