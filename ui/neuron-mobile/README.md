# Neuron Mobile

Continue a Neuron session from your phone, from anywhere. The session keeps running on your
computer; the phone follows it live, answers approvals and questions, and sends prompts. Everything
between the phone and the computer is end-to-end encrypted, and the Zero-X relay in the middle
only routes sealed frames.

- **Sessions**: every shared session on every paired computer, grouped by project, with
  *running*, *needs you*, *failed* and *new* badges.
- **A session**: the transcript in Markdown, reasoning that folds open, tool calls that open on
  their diff or output, approvals with a diff preview and *Deny with a note*, the agent's questions
  and task list, and Stop.
- **Computers**: pair by QR code, see who is online, forget a computer.
- **Account**: email, plan, today's usage, linked computers and signed-in devices, the *Needs me*
  and *Finished* notification switches, sign-out. It shows the plan and has no purchase UI.
- Light and dark themes (System by default), in Settings.

Expo SDK 58, React Native 0.88, expo-router. It shares the session reducer and remote protocol 1
with the desktop app (`ui/neuron-client`), so both show the same session at once.

## How it connects

```
phone ──wss──▶ Zero-X relay (one room per account) ◀──wss── computer (neuron engine --share)
```

1. The computer shows a QR code holding the relay's address, a one-time code and its public key.
2. The phone signs in to Zero-X (device code, approved in the browser) and scans it. Both ends
   derive a pair key with X25519 and HKDF; the relay sees only a hash of the code. The keys stay
   in the phone's secure storage (Keychain or Keystore).
3. Each time the phone opens a session, both ends make fresh keys, and every remote-protocol frame
   travels sealed with AES-256-GCM.
4. When the network drops, the phone reconnects with backoff and resumes from the last frame it
   got; when the relay cannot resume, it attaches again and takes a fresh snapshot.
5. When a session needs you (an approval waited 30 s) or finished, and the phone is not open on
   it, the relay sends a push that names the computer and nothing else.

The full contract, with test vectors for the Rust side and the relay, is
[`docs/RELAY_CONTRACT.md`](docs/RELAY_CONTRACT.md).

## Share a session through Zero-X

The relay is `wss://relay.zero-x.live/v1/relay` (zero-x.live `neuroncli/auth-server/src/relay.js`)
once it is deployed; the computer's side is `neuron engine --share`
(`rust/crates/neuron-share`). Until the engine serves its own session, it shares one a local Neuron
host serves:

```bash
# In ui/neuron-client: a host for your project (it prints its ws:// address).
bun run host --cwd <project> --token-file <file>
# Anywhere: share it. Needs `neuron login`; prints the QR code. Enter gives a new code.
neuron engine --share --host ws://127.0.0.1:<port>/neuron --host-token-file <file>
```

Then pair from the app as in step 5 below. To point `--share` at the dev relay instead, add
`--relay ws://127.0.0.1:8790/v1/relay` and set `NEURON_TOKEN` to a token from the dev relay's
`POST /dev/session`.

## Pair a phone locally

This package also has stand-ins for the relay and the computer under `dev/`, for work without a
network. You need Bun 1.3.

1. Install, here and in `ui/neuron-client`:

   ```bash
   bun install
   ```

2. Start the dev relay. It also serves device sign-in and the account API:

   ```bash
   bun run relay
   ```

   It listens on `127.0.0.1:8790`. For a phone on your Wi-Fi, add `--lan` and use your computer's
   LAN address below. Anyone on that network can approve sign-ins on the dev relay, so use `--lan`
   only on a network you trust.

3. Start a dev computer. It runs a scripted demo engine behind a local host, shares the session and
   prints a QR code and a pairing link:

   ```bash
   bun run share --relay ws://127.0.0.1:8790/v1/relay --name "Dev laptop"
   ```

   Add `--neuron --cwd <project>` to share your own `neuron engine` instead of the demo. Its id,
   keys and paired phones are kept in `~/.neuron-dev-share` (`--state` to move it), so the phone
   stays paired across restarts. The dev relay keeps pairings in memory: after restarting the relay,
   pair again.

4. Start the app:

   ```bash
   EXPO_PUBLIC_NEURON_GATEWAY_URL=http://127.0.0.1:8790 bun run start
   ```

   Open it in Expo Go, or press `w` for the web build (`bun run web`). On a phone, set the gateway
   to `http://<LAN address>:8790` instead.

5. In the app, go to **Computers → Pair a computer**. Scan the QR code, or paste the
   `neuron://pair?…` link the dev computer printed (the web build has no camera).
6. The app asks you to sign in. Tap **Open the approval page**, and approve the code on the dev
   relay's page as `dev@zero-x.live` (the dev computer signs in as that account).
7. The session appears under **Sessions**. Open it and send a prompt: the demo engine reasons,
   writes Markdown, reads a file and asks to edit it, with the diff.

Development builds pair with any relay; store builds only with relays on `zero-x.live`.

**Expo Go limits.** The camera, secure storage and everything else work in Expo Go. Push does not:
Expo Go on Android has no remote push since SDK 53, and push tokens need the EAS project that comes
with the store builds (WP6). The Account tab says why when push is not available.

## Build the app without store accounts

`.github/workflows/mobile-native.yml` builds, on every pull request that touches the app:

- **Android:** `scripts/build-android.sh` (a clean `expo prebuild`, then `./gradlew assembleRelease`
  for arm64-v8a and x86_64). Without a key the APK is debug-signed and named `-debug-signed`: it
  installs for testing (`adb install`, or open it on the phone after allowing installs from that
  app), but Play refuses it and Android will not update it over a build signed with another key.
  Run the workflow by hand with `signed` to sign it with the upload key from the `mobile-release`
  environment (`plugins/with-release-signing.js` reads it from `NEURON_ANDROID_*`).
- **iOS:** an unsigned Release build for the simulator (`xcrun simctl install booted Neuron.app`).

The same script builds locally on macOS or Linux with the Android SDK and JDK 17. On Windows the
React Native native build can exceed the 260-character path limit; build in CI or WSL.

## Tests

```bash
bun test
bun run typecheck
```

- `test/pairing.test.ts`: the pairing link parser (forms, every field, trusted relays, expiry).
- `test/crypto.test.ts`: the handshake against fixed vectors, both ends deriving the same keys, a
  relay swapping keys being caught, frames that open only with their key, channel, direction and
  counter, and the counters (duplicates, gaps, resends).
- `test/reconnect.test.ts`: the reconnect state machine (backoff, jitter, the stable reset, fatal
  close codes, offline and foreground), and the relay link on fake sockets and timers.
- `test/relay.integration.test.ts`: against the dev relay, end to end. Device sign-in, pairing, a
  session with a diff and a denial with a note, a dropped socket that resumes without a snapshot, a
  lost replay buffer that falls back to one, a push when nobody watches, a phone on another account
  that cannot pair, attach or inject, and no plaintext ever crossing the relay.
- `test/store.test.ts`: the app's store through sign-in, pairing, a restart, forget and sign-out.
- `test/markdown.test.ts`: the Markdown parser and the sessions list grouping.
- `test/connection.test.ts`: the LAN connection to a Bun host (still in Settings → Developer).
- `test/gateway.test.ts`: the gateway calls against the shapes zero-x.live answers with.
- `test/presence-vectors.test.ts`: the presence vectors `neuron engine --share` is tested against
  (`test/fixtures/presence-vectors.ts`) still match the reducer.
- `test/share-rust.integration.test.ts`: the Rust `neuron engine --share` against the dev relay, a
  host and the phone (set `NEURON_BIN` to a built `neuron`; skipped otherwise).

These are Jest-style tests (`describe`, `test`, `expect`) run by `bun test`, like the rest of the
repo's TypeScript packages.

## Layout

| Path | What |
|---|---|
| `app/` | expo-router routes: the tabs, `pair`, `sign-in`, `session/[host]/[share]`, `settings`, `lan` |
| `src/relay/` | the relay protocol (`contract.ts`), encryption (`crypto.ts`, `channel.ts`), the pairing link (`pairing.ts`), the reconnecting socket (`reconnect.ts`, `link.ts`), the phone's side (`phone.ts`, `transport.ts`), and the computer's side as a reference (`share-host.ts`, `presence.ts`) |
| `src/store/` | the app's state (`app.ts`), secure storage, the sessions list |
| `src/account/gateway.ts` | Zero-X sign-in, account, push registration |
| `src/push/push.ts` | Expo push token, channels, opening a tapped push |
| `src/components/`, `src/screens/` | the transcript, Markdown, diffs, approvals, sign-in |
| `dev/` | the dev relay and gateway (`relay.ts`, `relay-room.ts`), the dev computer (`share.ts`), the demo engine |
| `docs/RELAY_CONTRACT.md` | the contract WP5 (relay) and WP1 (`--share`) implement |
| `plugins/with-release-signing.js`, `scripts/build-android.sh` | Android release builds without EAS |

Store builds (EAS, TestFlight, Play) are WP6's, with `eas.json`.
