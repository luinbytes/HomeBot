# Security Policy

HomeBot is pre-release and does not yet accept security claims for a stable supported version. Please do not expose development builds beyond loopback.

Report suspected vulnerabilities privately through GitHub Security Advisories once the public repository is available. Do not include live credentials or unrelated private data. For urgent reports, include the affected commit, platform, reproduction steps, impact, and a minimal proof of concept.

HomeBot treats server-side permission enforcement, token handling, secret storage, path confinement, process execution, plugin isolation, and safe Git operation as release-blocking boundaries. See [docs/security.md](docs/security.md) for the threat model.
