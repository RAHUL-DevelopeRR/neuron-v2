# Neuron release progress

Updated: 2026-10-06

## Completed

- Fixed the Neuron MCP stdio transport default to use newline-delimited JSON-RPC and initialize servers correctly.
- Kept the legacy Content-Length protocol available for compatible servers and made the legacy test fixtures select it explicitly.
- Corrected the Unix test-only MCP initialization timeout so parallel CI has enough time to start fixture processes.
- Improved gateway authentication/quota error handling and the CLI-to-gateway behavior.
- Merged and deployed the Zero-X gateway quota fix. The live API readiness check passed and the model catalog returned two Cloudflare models.
- Local Windows full workspace tests and strict Clippy passed before the latest test-fixture-only adjustments. Focused MCP tests (54) and strict Clippy pass after those adjustments.
- npm account login is verified as `zero-x.live`; npm 2FA is enabled. Package dry-run succeeds for `@zero-x.live/neuron@6.2.5`.

## Current state

- Neuron changes are on branch `codex/gateway-client-fixes`, associated with PR #3: https://github.com/RAHUL-DevelopeRR/neuron-v2/pull/3
- The latest recorded GitHub CI run failed on Unix MCP fixture startup/transport behavior and passed Windows, Clippy, formatting, React TUI, and docs checks. The Unix fixture/timeout fixes are now pushed in commit `fcc2360`; a fresh cross-platform CI run is pending.
- A manual prompt against the live gateway returned the expected structured `429 insufficient_quota` response because the account's daily allocation was exhausted. Earlier authenticated live inference and model-health probes succeeded; this latest CLI attempt did not produce an inference answer.
- The backend health checker and provider adapters are present. Cloudflare is the only provider configured with live server credentials; Groq, OmniRoute, OpenRouter, NVIDIA NIM, Gemini, and AWS Bedrock still need server-side credentials and quota policy before live routing can be verified.

## Remaining work

1. Push this progress note and the Unix MCP fixture/timeout fixes; wait for Linux, macOS, and Windows CI to pass.
2. Merge PR #3 and create the `v6.2.5` release tag; verify the Linux x64, macOS arm64, and Windows x64 binaries and SHA-256 assets.
3. Configure npm trusted publishing for package `@zero-x.live/neuron`, repository `RAHUL-DevelopeRR/neuron-v2`, workflow `.github/workflows/release.yml`, publish permission only. npm requires a browser one-time verification for this account-level grant; no token or OTP should be shared in chat.
4. Enable the repository's `ENABLE_NPM_PUBLISH` variable, publish from the verified release workflow, then install the published package and check `neuron --version`, authentication, model listing, and a live prompt.
5. Restore daily Neuron allocation before repeating the live prompt test. Validate provider health and inference separately as credentials are provisioned for each vendor.
6. PyPI publishing remains paused until account MFA recovery is available.
