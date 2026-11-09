# @neuron/client

The code the Neuron front ends share. It speaks `neuron engine --stdio` and the remote protocol
built on it. Nothing here runs the agent: the Rust engine does.

- `src/protocol.ts`: the JSON-lines contract, mirrored from
  `rust/crates/neuron-tui/src/protocol.rs`. `test/protocol.test.ts` fails when the Rust enums
  gain or lose a variant. `parseClientMessage` is the gate for frames from remote peers (a paired
  phone): every engine command is decided there on purpose, in `PEER_PASSES` or `PEER_REFUSED` in
  that test, so a new command stays refused until someone says a phone may send it. Passing it is
  not permission: `src/node/host.ts` decides every command again in `HOST_POLICY` (a new command
  fails `bun run typecheck` until it is listed there) and refuses more, for every peer, local or
  not: slash commands outside `REMOTE_SLASH`, a rewind that is not a dry run.
- `src/session.ts`: one reducer from engine events and user actions to a transcript. It covers
  messages, reasoning, tool calls with their state, permission requests tied to their call,
  questions, sign-in and usage.
- `src/tools.ts` and `src/diff.ts`: what a tool call shows. That is its kind, a title, the diff it
  will make (from its input), the diff it made (from the engine's result) and its output.
- `src/controller.ts`: a session over a transport, with `getState`/`subscribe` for React's
  `useSyncExternalStore`.
- `src/remote.ts`: the remote protocol (one engine, many clients) and a WebSocket transport that
  runs in browsers and React Native.
- `src/theme.ts`: the brand palette from the NeuronCLI 6.2.5 terminal UI.
- `src/node/` (Bun only): the stdio transport, the host and the mock-service helpers.

## Run

```bash
bun test
```

Integration tests run the real engine against the mock Anthropic service. Build both first in
`rust/`: `cargo build -p rusty-claude-cli -p mock-anthropic-service`. Without them, those tests
are skipped.

```bash
bun run host --mock
```

That serves an engine running against the mock to remote clients. Point the desktop app (in a
browser) or the mobile app at the printed URL, with the token from the printed token file.
`--lan` listens on the network for a phone, and `--show-token` prints the token. Without `--mock`
it runs your own `neuron` in `--cwd`.

A host holds every client to the same walls, whether it comes from this machine or not: no commit,
rewind, connector or permission-mode command, only the read-only slash commands, and `resume` by
session id. The desktop app in a browser needs more (a new session and the permission mode are slash
commands), so start the host with `--owner-on-loopback` for it. That lets a client on this machine use
every slash command. Never combine it with `neuron engine --share`: the phone behind that bridge also
reaches the host from 127.0.0.1, and the host cannot tell it from you.
