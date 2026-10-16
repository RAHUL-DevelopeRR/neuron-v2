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
- **Your Zero-X login.** `neuron auth login` (and `--with-token`) stores the session
  token (`ses_…`) or API key (`nzx_…`), with its expiry and plan, in the operating
  system's credential store on macOS (Keychain) and Windows (Credential Manager). On
  Linux, when that store is unavailable, or when the Neuron home is redirected
  (`NEURON_HOME`, or a `HOME`/`USERPROFILE` other than your account's), it is written
  in plain text to `~/.neuroncli/credentials.json` instead, readable only by you (mode
  0600) on Linux and macOS and protected by your user profile's permissions on
  Windows; Neuron warns once when it falls back to the file.
  `NEURON_CREDENTIAL_STORE=file|keychain` forces one backend. Anyone who can read that
  file, or who can run code as you, can use the credential, so treat
  `~/.neuroncli` like a password file and keep it out of shared backups, synced
  folders, container images and support bundles. A `gateway.enc` file left by an
  older release is migrated once into the current store and deleted.
- **If the credential leaks**, run `neuron auth logout` on that machine, which deletes
  the local copy and, for a session token, revokes that session on the gateway. If you
  are not sure which session leaked, revoke every session from the Zero-X dashboard.
  Sessions also expire on their own (30 days by default); API keys are revoked in the
  dashboard.
- **Personal API keys** (`nzx_…`) are for CI and headless machines. Keep them in your
  CI secret store, never in a repository, and revoke them in the dashboard when no
  longer needed. They can call the model API and read usage, but cannot create keys
  or change billing.
- **Environment variables and subprocesses.** `NEURON_TOKEN` and bring-your-own
  provider keys such as `OPENAI_API_KEY` are read from the environment; Neuron does not
  save them, and it never exports the stored login or a provider key into its own
  environment: the credential is handed to the HTTP client in memory. Shell commands
  (bash, PowerShell), hooks, plugin tools and MCP servers that Neuron starts do not
  inherit `NEURON_TOKEN`, `NEURON_SESSION_TOKEN` or any variable whose value starts
  with `ses_` or `nzx_` (a value set explicitly in an MCP server's `env` still
  applies). Provider keys you exported yourself, such as `OPENAI_API_KEY`, are still
  inherited by those processes, and everything they run can read your files, including
  the credentials file above. Approve commands with that in mind (see
  [Running untrusted code](#running-untrusted-code)).
- **Provider keys you bring** are sent only to the endpoint you configured for them.

## Running untrusted code

Neuron runs commands and edits files on your behalf. Its permission modes
([USAGE.md](USAGE.md#permission-modes)) limit what it may do without asking: a shell
command runs without a prompt only when every part of the command line is classified
as read-only, and it asks you to trust a directory before working in it. Prompt
injection from files, web pages or tool output can steer the model, so review commands
before approving them,
and use `read-only` or a container for code you do not trust.
`--dangerously-skip-permissions` removes every check and belongs only in disposable
environments.

## Release integrity

- Release binaries are built by GitHub Actions from the release tag. Each has a
  `.sha256` file, the release has `SHA256SUMS`, and every binary has a GitHub build
  provenance attestation:
  `gh attestation verify <file> --repo RAHUL-DevelopeRR/neuron-v2`.
- The assets and checksums of a published release are never rebuilt or replaced; a
  fix ships as a new version. A checksum that does not match is therefore worth
  reporting.
- `install.sh`, `install.ps1` and both package launchers verify the SHA-256 checksum
  before a binary is installed or cached, and refuse non-HTTPS download mirrors.
- The binaries are not yet code-signed (Windows) or notarized (macOS).
- `THIRD_PARTY_NOTICES.md` on each release lists the Rust crates compiled into the
  binary with their licenses.
