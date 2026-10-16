# Security policy

## Reporting a vulnerability

Report vulnerabilities privately through GitHub:
<https://github.com/RAHUL-DevelopeRR/neuron-v2/security/advisories/new>. Please do
not open a public issue, pull request or discussion for a suspected vulnerability.
If that form is unavailable, open an issue that asks for a private contact and leave
out the details.

Include what you found, the version (`neuron --version`), your platform and install
method, and the steps to reproduce. We aim to acknowledge a report within five working
days, keep you informed while we work on it, and agree on a disclosure date with you.
Credit is given in the advisory unless you prefer otherwise.

In scope: the `neuron` CLI and its Rust workspace, the npm and Python launchers, the
install scripts and the release pipeline in this repository. Report problems with the
Zero-X gateway or website the same way; we route them to the gateway maintainers.

## Supported versions

| Version | Supported |
| --- | --- |
| 6.3.x | Yes |
| Earlier than 6.3.0 | No; this includes the npm 6.2.0 and PyPI 6.2.4 (and earlier) packages, which were built outside the release pipeline |

Security fixes are released as a new patch version; upgrade to get them.

## How Neuron handles credentials

- **Provider keys stay on the gateway.** When you use Zero-X, the API keys for Groq,
  Google, Cloudflare and other upstream providers live only on the gateway. The CLI,
  the npm package and the Python package do not contain them.
- **Your Zero-X session.** `neuron auth login` stores a session token (`ses_…`) in
  `~/.neuroncli/gateway.enc`. The file is encrypted with a key derived from the
  machine and user identity. That stops the file from working when copied to another
  machine; it does not protect it from other software running under your account.
  Treat the directory as secret. Sessions expire (30 days by default), `neuron auth
  logout` revokes the current one, and the Zero-X dashboard can revoke every session at
  once.
- **Personal API keys** (`nzx_…`) are for CI and headless machines. Keep them in your
  CI secret store, never in a repository, and revoke them in the dashboard when no
  longer needed. They can call the model API and read usage, but cannot create keys
  or change billing.
- **Environment variables.** `NEURON_TOKEN` and bring-your-own provider keys such as
  `OPENAI_API_KEY` are read from the environment. Neuron does not export your gateway
  session into the environment and removes credential variables from the environment
  of the shell commands, hooks and MCP servers it starts. Other programs you run in the
  same shell can still read variables you set there yourself.
- **Provider keys you bring** are sent only to the provider they belong to.

## Running untrusted code

Neuron runs commands and edits files on your behalf. Its permission modes
([USAGE.md](USAGE.md#permission-modes)) limit what it may do without asking, and it
asks you to trust a directory before working in it. Prompt injection from files, web
pages or tool output can steer the model, so review commands before approving them,
and use `read-only` or a container for code you do not trust.
`--dangerously-skip-permissions` removes every check and belongs only in disposable
environments.

## Release integrity

- Release binaries are built by GitHub Actions from the release tag. Each has a
  `.sha256` file, the release has `SHA256SUMS`, and every binary has a GitHub build
  provenance attestation:
  `gh attestation verify <file> --repo RAHUL-DevelopeRR/neuron-v2`.
- `install.sh`, `install.ps1` and both package launchers verify the SHA-256 checksum
  before a binary is installed or cached, and refuse non-HTTPS download mirrors.
- The binaries are not yet code-signed (Windows) or notarized (macOS).
- `THIRD_PARTY_NOTICES.md` on each release lists the Rust crates compiled into the
  binary with their licenses.
