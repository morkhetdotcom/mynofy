# Security policy

mynofy is a prototype. No long-term supported release line or security response
SLA is currently published. Use the current source and review its documented
trust and durability boundaries before deployment.

## Report a vulnerability privately

Use GitHub's **Report a vulnerability** action under the public repository's
**Security** tab if private vulnerability reporting is enabled. If it is not
available, ask a maintainer in a public issue for a private reporting channel
without disclosing exploit details. No dedicated security email is currently
published.

Include the affected version, platform, prerequisites, minimal reproduction,
expected security property, observed behavior, and impact. Use synthetic keys and
data. Do not publish working attacks against deployments, private credentials,
or customer content.

For ordinary bugs that do not expose a security issue, open an issue or pull
request following [CONTRIBUTING.md](CONTRIBUTING.md).

See the [security model](docs/security.md) for the distinction between client
identity, topic authorization, service trust, local OS access, and persistence.
