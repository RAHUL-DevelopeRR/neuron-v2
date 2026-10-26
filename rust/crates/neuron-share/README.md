# neuron-share

`neuron engine --share`: shares a Neuron session with the Neuron phone app through the Zero-X
relay, end-to-end encrypted. The session keeps running on the computer. This crate is the Rust
port of the apps' reference, `ui/neuron-mobile/src/relay/share-host.ts`, and speaks
`neuron-relay/1` (`ui/neuron-mobile/docs/RELAY_CONTRACT.md`; the relay is `RelayRoom` in
zero-x.live `neuroncli/auth-server/src/relay.js`).

```bash
# 1. A local Neuron host serving the session over remote protocol 1 (for now the Bun host):
cd ui/neuron-client && bun run host --cwd <project> --token-file <file>
# 2. Share it (needs `neuron login`; prints a QR code and a pairing link):
neuron engine --share --host ws://127.0.0.1:<port>/neuron --host-token-file <file>
```

`--relay` (or `NEURON_RELAY_URL`) points at another relay, such as the dev relay
(`ws://127.0.0.1:8790/v1/relay`, with `NEURON_TOKEN` from its `/dev/session`). `--name` names the
computer, `--no-pair` skips the first code, and Enter prints a new one.

What it does:

- connects out to the relay with the Zero-X session in the `Authorization` header, joins as a
  computer, keeps the socket alive and reconnects with backoff (stops on 4400/4401/4403/4409);
- offers one-time pairing codes and answers pairings, checking the phone's proof;
- accepts channels from paired phones and bridges each to the local host, letting through only
  `hello` (with the local token put in), `send` (not `trust_workspace`, `shutdown` or
  `capabilities`), `ping` and the read-only `rpc` methods (`fs.list`, `fs.read`, `fs.search`,
  `git.status`, `git.diff`); terminals and engine restarts stay on the computer;
- follows the session as a client of the local host and sends its sealed presence, plus a
  contentless `notify` when it starts waiting for the user or a turn ends;
- keeps its identity (host id, X25519 secret, presence key), paired phones and share ids in
  `<neuron home>/remote.json`.

Tests: the contract's crypto vectors (`src/crypto.rs`), the host without I/O (`src/host.rs`), presence
against vectors from the TypeScript reducer (`tests/presence_vectors.rs`, fixture from
`ui/neuron-mobile/test/fixtures/presence-vectors.ts`), and the whole share over real sockets
(`tests/share_flow.rs`). `ui/neuron-mobile/test/share-rust.integration.test.ts` runs the built
binary against the TypeScript dev relay, host and phone (`NEURON_BIN=...`).

Not yet: the engine serving remote protocol 1 itself (so the TUI and the desktop app can share
their own session with *Continue on phone*), and resuming channels after the computer's socket
drops (phones attach again instead).
