# Security

## Reporting a vulnerability

Please report security problems privately: on GitHub, open the repository's **Security** tab and choose **Report a vulnerability**. Don't open a public issue for them. You'll get a reply within a week.

Useful to include: the Meridian version (Meridian → About Meridian), your macOS version, what an attacker could do, and steps to reproduce.

## How Meridian handles secrets

- API keys and the SEC EDGAR contact are stored only in the macOS Keychain. They are never written to the data directory, logs, crash reports or settings files.
- Keys are sent only to the source they belong to, over HTTPS (FRED's API takes its key as a URL parameter; Meridian strips it from anything it records, such as source links).
- Releases are signed with a Developer ID certificate, notarized by Apple, and built with the Hardened Runtime.

## Supported versions

Fixes go into the latest release. Meridian checks for updates (Settings → General); please update before reporting.
