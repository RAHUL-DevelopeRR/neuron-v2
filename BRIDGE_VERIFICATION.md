# Bridge repair verification

Workspaces: C:\Users\DELL\neuron-v2-fixes and C:\Users\DELL\zero-x.live.

## Changes

The CLI signs in through the Zero-X account page using a random loopback port and state. Only a gateway session reaches the client; provider keys stay on the server. Provider selection uses an explicit configured catalog. Node and Cloudflare Worker entry points share the same request handling. Quotas reserve tokens atomically and reconcile reported usage.

Parallel streamed tool calls preserve their IDs and arguments. Incomplete or malformed tool streams fail before execution. Chain/power commands use the configured gateway. The JavaScript launcher forwards to the native terminal runtime. Dashboard loading, retries, session reuse and login callback handling are repaired. CI tests the actual default branch and builds Neuron release artifacts for Windows, Linux and macOS.

Web-fetch title matching preserves Unicode byte offsets. HTTP test cleanup preserves the original panic, and agent fixtures avoid live background inference. The latest-session fixture uses an isolated store instead of changing process-wide CWD.

## Verification

- Gateway: all eight npm behavior checks passed; production npm audit reported zero vulnerabilities. Both account/quota migrations passed isolated PostgreSQL/PGlite validation. Provider inference used mocks.
- Rust: full serial workspace tests passed with 992 passed, 21 ignored and zero failures (Windows). This includes API 132, runtime 438, CLI 189 and tools 82 unit tests, client integrations and doc tests. The real CLI subprocess preserves two interleaved tool calls and file results; the gateway chain adapter regression passed.
- TypeScript build, native launcher --version, documentation source-of-truth check, Python shim recursion check and Rust formatting passed. Final strict workspace Clippy (--all-targets -- -D warnings) passed with exit zero.
- Earlier parallel runs exposed temporary-directory cleanup, nonblocking HTTP fixture sockets and shared PATH races; these fixtures are corrected. CI and documented verification run tests serially because fixtures mutate process-wide environment and working directories. Linux/macOS execution awaits CI.

## Deployment and limits

No push, production migration, Worker deployment or release publication has been performed. Apply accounts migration, then quota reservations migration before deploying the matching Worker; configure provider credentials and allowlists. See C:\Users\DELL\zero-x.live\neuroncli\auth-server\GATEWAY_SETUP.md.

Desktop/browser automation timed out or reported User unavailable. Visual browser QA, real account sign-in and deployed provider inference remain unverified. Free provider availability depends on credentials, quotas and the configured catalog.

The inherited vault encrypts sessions with a deterministic machine-derived key. It is not backed by the operating system credential manager.
