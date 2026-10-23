# Neuron relay contract (`neuron-relay/1`)

How a phone reaches a Neuron session on a computer from anywhere, while the session keeps running
on the computer. Three parties implement this document:

| Party | Where | Status |
|---|---|---|
| Phone | `ui/neuron-mobile/src/relay/` (`phone.ts`, `transport.ts`, `link.ts`, `crypto.ts`, `pairing.ts`) | built (WP4) |
| Relay | zero-x.live: a Durable Object per account, `RelayRoom` (A11), and the push service (A12) | WP5. A stand-in runs here: `dev/relay-room.ts`, served by `dev/relay.ts` |
| Computer | `neuron engine --share` (R1, Rust) | WP1. The TypeScript reference is `src/relay/share-host.ts`, run by `dev/share.ts` |

The source of truth for frame shapes is `src/relay/contract.ts`; for the cryptography,
`src/relay/crypto.ts` with the vectors in §3.7. This document explains them and lists what each
party must do. It refines `docs/NEURON_APPS_PLAN.md` §5; §9 lists where it differs.

## 1. Overview

```
 phone ──wss──▶ relay (one room per Zero-X account) ◀──wss── computer ──loopback──▶ session host
        sealed data frames, routing headers only          (neuron engine, or NeuronHost in dev)
```

- Each device keeps **one WebSocket** to the relay: the phone for all its paired computers on that
  relay, the computer for all the sessions it shares.
- The relay authenticates both ends with a Zero-X session (`ses_…`) and puts them in the
  **account's room**. A device on another account is in another room and cannot address anything
  here (§4.6).
- Inside a **channel** (one phone attached to one shared session), the phone and the computer run
  **remote protocol 1** (`ui/neuron-client/src/remote.ts`: `hello`, `send`, `rpc`, `snapshot`,
  `action`, …) unchanged. Every one of those frames is sealed end to end (§3). The relay sees only
  routing fields: channel ids, counters, device ids, sizes and times.
- The relay keeps a **replay buffer** of sealed computer-to-phone frames per channel, so a phone
  whose socket dropped resumes from the last frame it opened instead of fetching a snapshot (§5).
- The computer tells the relay, with a contentless `notify`, when a session needs the user or
  finished. The relay pushes when no phone is attached (§7).

## 2. The pairing link (QR code)

*Continue on phone* (desktop, D26) and `/phone` (terminal) show a QR code of:

```
neuron://pair?v=1&r=<relay URL>&h=<host id>&c=<code>&k=<host key>&n=<name>&u=<account tag>&e=<expiry>
```

| Field | Required | Value |
|---|---|---|
| `v` | yes | `1` |
| `r` | yes | the relay's WebSocket URL, e.g. `wss://relay.zero-x.live/v1/relay`. No credentials, query or fragment. `ws://` only for loopback and private-network hosts. |
| `h` | yes | the computer's host id: 8–64 of `[A-Za-z0-9_-]` |
| `c` | yes | the one-time pairing code: 16 random bytes, base64url (22 characters). **Never sent to the relay.** |
| `k` | yes | the computer's X25519 public key: 32 bytes, base64url (43 characters) |
| `n` | no | the computer's name, at most 64 characters |
| `u` | no | the account tag from the computer's `joined` frame (§4.3), so the phone can say "different account" before trying |
| `e` | no | expiry, Unix seconds |

The same fields may travel in the fragment of an https link, which browsers never send to a server:
`https://neuron.zero-x.live/pair#v=1&r=…`. The phone parses both (`parsePairingLink`), checks every
field, and in store builds accepts only relays on `zero-x.live` and its subdomains. Development
builds accept any relay, so a phone can pair with `dev/relay.ts` on the developer's machine.

The phone signs in to Zero-X with the device-code flow (`/auth/device/code` and
`/auth/device/token`, `docs/GATEWAY_CONTRACT.md`), approving in the browser, before it pairs. The
gateway is `https://zero-x.live` for Zero-X's relay, or the relay's own origin for a dev relay.

## 3. Cryptography

All binary values are **base64url without padding**. Strings are UTF-8. `‖` is concatenation.
Primitives: X25519, HKDF-SHA256, HMAC-SHA256, SHA-256, AES-256-GCM (12-byte nonce, 16-byte tag).
`L` is the label `neuron-relay/1`.

### 3.1 Pair id

`pair_id = b64u(SHA-256(utf8(L ‖ " pair-id") ‖ code_bytes)[0..16])`. The relay routes the pairing by
this; it cannot recover the code from it.

### 3.2 Pair key

The phone makes a key pair `(p, P)`; the computer has `(h, K)` (`K` is in the QR).

```
shared   = X25519(p, K)            (= X25519(h, P) on the computer)
pair_key = HKDF-SHA256(ikm = shared, salt = code_bytes,
                       info = utf8(L ‖ " pair") ‖ K ‖ P, length = 32)
```

An all-zero `shared` (a small-order key) is refused. Both ends keep `pair_key`: the phone in secure
storage, the computer in its Neuron config. The code is spent after one pairing attempt.

### 3.3 Proofs

`proof(role) = b64u(HMAC-SHA256(pair_key, utf8(L ‖ " " ‖ role ‖ "-proof " ‖ pair_id))[0..16])`,
with `role` = `phone` or `host`. The phone sends `proof(phone)` in `pair`; the computer checks it
(constant time) and answers `proof(host)` in `paired`, which the phone checks. The relay knows only
`pair_id`, so it can make neither.

### 3.4 Pairing secrets

`paired.sealed` carries, sealed as a blob (§3.6) with `key = HKDF-SHA256(ikm = pair_key, salt = none,
info = utf8(L ‖ " pair-secrets"), 32)` and `aad = L ‖ " pair-secrets|" ‖ pair_id`:

```json
{ "presence_key": "<32 bytes, b64u>", "name": "Rahul's laptop" }
```

`presence_key` is the computer's own random key that seals its presence blobs for all its phones.

### 3.5 Channels

Each attach makes fresh ephemeral keys: the phone `(a, A)` sends `A` in `open`, the computer `(b, B)`
answers `B` in `accept`.

```
okm  = HKDF-SHA256(ikm = X25519(a, B), salt = pair_key,
                   info = utf8(L ‖ " channel " ‖ ch) ‖ A ‖ B, length = 64)
p2h  = okm[0..32]     seals phone → computer
h2p  = okm[32..64]    seals computer → phone
confirm = b64u(HMAC-SHA256(h2p, utf8(L ‖ " accept " ‖ ch))[0..16])     sent in accept
```

The phone checks `confirm` before it uses the channel. A relay that swaps `A` or `B` does not know
`pair_key`, so it cannot derive the keys or a valid `confirm`.

**Frames.** Each direction counts frames from `n = 0`, one up per frame.

```
nonce = 0x00000000 ‖ uint64_be(n)                         (12 bytes)
aad   = utf8(L ‖ "|" ‖ ch ‖ "|" ‖ dir)                    dir = "p2h" or "h2p"
c     = b64u(AES-256-GCM-Seal(key_dir, nonce, aad, utf8(inner frame JSON)))
```

A receiver takes only `n = last + 1`: a lower `n` is a duplicate and is dropped, a higher one is a gap
(the phone asks to `resume`), and a frame that does not open ends the channel. The inner frame is one
remote protocol 1 frame as JSON text.

### 3.6 Blobs

Presence and the pairing secrets use a random nonce: `blob = b64u(nonce(12 random bytes) ‖
AES-256-GCM-Seal(key, nonce, utf8(aad), utf8(json)))`. Presence uses `key = presence_key` and
`aad = L ‖ " presence|" ‖ host_id ‖ "|" ‖ share_id`.

### 3.7 Test vectors

From `test/crypto.test.ts` (which checks every line). Inputs:

| Name | Value |
|---|---|
| host secret `h` | `AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA` (bytes 1…32) |
| phone secret `p` | `QEFCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaW1xdXl8` (bytes 0x40…0x5f) |
| host ephemeral `b` | `gIGCg4SFhoeIiYqLjI2Oj5CRkpOUlZaXmJmam5ydnp8` |
| phone ephemeral `a` | `oKGio6SlpqeoqaqrrK2ur7CxsrO0tba3uLm6u7y9vr8` |
| code | `wMHCw8TFxsfIycrLzM3Ozw` (bytes 0xc0…0xcf) |
| ch | `channel-0000000001` |

Outputs:

| Name | Value |
|---|---|
| `K` | `B6N8vBQgk8i3VdwbEOhstCY3StFqqFPtC9_AsrhtHHw` |
| `P` | `eaYx7t4b-cmPEgMs3q3Q56B5OY_HhriMyEbsia-FpRo` |
| pair_id | `xMm4z_g3jDfuYMKwidZHBg` |
| pair_key | `4D5wivpGleJpH83D0sn-YabpzY7zC8PG-6GXqDdUalk` |
| proof(phone) | `cbwHSSkvuXNM06i3rkMJkw` |
| proof(host) | `A9pJGL2ru9oPNO9AkqUO9A` |
| pair-secrets key | `ml4UAMqUKZdHGLgVhmj2Kn-ArGEsmn2CFyoqF-3mJmo` |
| p2h | `N174o0i91fATCDiCxybPvW1IVSik5YrI1VXfwhySPl8` |
| h2p | `CqAUf_LDYB8oW3HicMS7OV03SzZQgPpuGpyutLS3Ii4` |
| confirm | `WmXZ4JoZr6XvkScveQ9llQ` |
| seal(p2h, n=0, `{"t":"ping"}`) | `FHfS93E8PCTjaHu3pzTqB3pme1Hi1W-Z3o793A` |
| seal(h2p, n=1, `{"t":"pong"}`) | `7-VzfWIbvw7QbuxBgJrSWrbWheJ1f2hUUvjEfA` |

## 4. The relay socket

### 4.1 Endpoint and credentials

`wss://relay.zero-x.live/v1/relay` (the dev relay: `ws://127.0.0.1:8790/v1/relay`). The QR carries
the full URL, so the path can change without an app release; the host must stay on `zero-x.live`.

The client offers two WebSocket subprotocols: `neuron-relay.v1` and `neuron-auth.<ses_ token>`.
Browsers cannot set an `Authorization` header on a WebSocket, and the Worker needs the account
before it picks the Durable Object, so the token cannot wait for a first frame. Native clients
(the CLI) may send `Authorization: Bearer ses_…` instead. Rules for the relay:

- answer with `Sec-WebSocket-Protocol: neuron-relay.v1` (browsers fail the handshake otherwise);
- never log either header, nor any frame body;
- refuse `nzx_` API keys (close `4403`), and unknown or expired sessions (close `4401`). Accept the
  upgrade and then close: a plain HTTP 401 looks like a network error to a browser, which would
  make the phone retry forever instead of asking the user to sign in.
- tokens are `ses_` plus base64url characters, which are valid subprotocol characters.

### 4.2 Close codes

| Code | Meaning | Client |
|---|---|---|
| 4400 | first frame was not `join`, or a protocol violation | stop: update the app |
| 4401 | no session, or expired or revoked | stop: sign in again |
| 4403 | the credential may not use the relay (API key, suspended account) | stop |
| 4408 | no `join` within 10 s | reconnect |
| 4409 | the same device joined again on another socket | stop (this socket was replaced) |
| 4429 | over a limit (devices, frame rate or size) | reconnect after the longest backoff |
| other | network, relay restart | reconnect with backoff |

### 4.3 Join and keepalive

The first frame is `{"t":"join","role":"host"|"phone","device":{"id","name","platform"}}`. The
relay answers `{"t":"joined","relay":1,"role","device_id","account":{"tag"}}`, where `tag` is the
first 16 hex characters of `SHA-256("neuron-relay/1 account|" ‖ account_id)`.

Keepalive is the exact text `{"t":"ping"}`, answered with the exact text `{"t":"pong"}`, so a
Durable Object can use `setWebSocketAutoResponse` and stay hibernated. Clients ping after 25 s
without a frame and drop a socket that does not answer within 10 s.

### 4.4 Frames: computer → relay

| Frame | Relay does |
|---|---|
| `share {share_id, sealed}` | stores the presence blob; sends `presence` to the computer's paired phones that are online |
| `unshare {share_id}` | forgets it; sends `presence` with `sealed: null` |
| `pair_offer {pair_id, ttl_s}` | keeps the offer (≤ 600 s, ≤ 5 live per computer); answers `pair_offer_ok {pair_id, expires_at}` (ms) |
| `pair_cancel {pair_id}` | drops the offer |
| `paired {pair_id, phone_id, proof, sealed}` | records the pairing (in storage), drops the offer, sends the phone `paired {pair_id, host_id, name, proof, sealed}`, then `host` and the computer's `presence` |
| `pair_denied {pair_id, phone_id, code}` | sends the phone `pair_denied {pair_id, code}` |
| `accept {ch, e, confirm}` / `reject {ch, code}` | passes it to the channel's phone; `reject` drops the channel |
| `data {ch, n, c}` | keeps it in the channel's replay buffer; passes it on if the phone is attached |
| `close {ch, code?}` | drops the channel; tells the phone |
| `notify {share_id, kind}` | §7 |

### 4.5 Frames: phone → relay

| Frame | Relay does |
|---|---|
| `list` | answers `hosts {hosts: [{host_id, name, online, shares: [{share_id, sealed, at}]}]}`, only computers paired with this phone |
| `pair {pair_id, pub, proof, name}` | passes `pair {pair_id, phone_id, pub, proof, name}` to the offering computer; unknown or expired: `error {code: "pair_not_found", pair_id}` |
| `open {ch, host_id, share_id, e}` | needs a pairing (`error not_paired`) and the computer online (`error host_offline`); passes `open {ch, phone_id, share_id, e}` on |
| `resume {ch, after}` | §5 |
| `data {ch, n, c}` | passes it to the computer; remembers the highest `n` passed |
| `close {ch}` | drops the channel; tells the computer |
| `unpair {host_id}` | drops the pairing and its channels; tells the computer `unpaired {phone_id}` |

The relay also sends phones `host {host_id, name, online}` when a paired computer joins or leaves,
and `presence {host_id, share_id, sealed, at}` as computers update. It sends computers
`peer {ch, state: "away"|"back"}` when a phone's socket drops or resumes.

Errors are `{"t":"error","code","message","ch"?,"pair_id"?}`; `message` is for logs, the app
words its own.

### 4.6 Accounts

One room per account. Every lookup (offers, pairings, channels, devices) is inside the room, so a
device on another account cannot pair with, attach to or send frames into this one: it gets
`pair_not_found`, `not_paired` or `unknown_channel` (`test/relay.integration.test.ts` checks all
three). Pairings are per `(host_id, phone_id)` and must survive a relay restart (Durable Object
storage); everything else may be in memory.

## 5. Channels, the replay buffer and resume

- A channel lives from `open` until a `close`, a `reject`, the computer leaving, or the phone being
  away for 10 minutes.
- The relay keeps the channel's last **512 computer-to-phone frames (at most 4 MB)**, whether or
  not the phone is attached: frames in flight when a socket drops are lost on the wire, not in the
  buffer.
- After the phone's socket reconnects and joins, it sends `resume {ch, after}` for each open
  channel, where `after` is the last `n` it opened (−1 for none). The relay answers
  `resumed {ch, received}` (the highest phone `n` it passed to the computer), then replays every
  buffered frame with `n > after`, in order. The phone sends again what it sealed after `received`.
- If the buffer no longer holds `after + 1` (it overflowed, or the relay restarted), the relay drops
  the channel, tells the computer `close {ch, code: "lost"}`, and answers
  `resume_failed {ch, code: "gap"}`. The phone opens a new channel, and the host's fresh
  `snapshot` replaces its state. An unknown channel answers `resume_failed {code: "unknown_channel"}`.
- When the **computer's** socket drops, its channels end (`close {ch, code: "host_gone"}` to
  phones) and phones attach again once it is back. Resuming on the computer's side is P2.

## 6. Presence and the sessions list

A shared session's presence (inside its sealed blob):

```json
{ "v": 1, "project": "neuron-v2", "title": "Fix the build", "branch": "main", "model": "glm-5.2",
  "status": "running" | "needs_me" | "idle" | "failed", "last_seq": 412, "updated_at_ms": 1792769000000 }
```

`status` is `needs_me` while a permission request or a question waits, `running` during a turn,
`failed` when the engine exited or the last turn ended in an error, and `idle` otherwise
(`src/relay/presence.ts`). `last_seq` is the remote protocol `seq` of the session's latest action;
the phone shows a session as unread when it is above the last `seq` the user saw there. The
computer sends `share` at most a few times a second and only when the blob's content changed.

## 7. Notify and push

The computer sends `notify {share_id, kind}` when a session's status becomes `needs_me`
(`kind: "needs_me"`), or goes from `running` to `idle` or `failed` (`kind: "finished"`). It carries
nothing else. The relay:

- ignores it while any phone is attached to that session;
- pushes `finished` at once, and `needs_me` after **30 s**, cancelled if a phone attaches first;
- pushes the same session and kind at most once a minute;
- pushes to the computer's paired phones that registered a token, and only for the switches they
  turned on.

Push payload (Expo push API), with no session content:

| kind | title | body | Android channel |
|---|---|---|---|
| `needs_me` | Neuron needs you | A session on <computer name> is waiting for your answer. | `needs-me` |
| `finished` | Neuron finished | A session on <computer name> finished its turn. | `finished` |

`data: {"neuron": {"host_id", "share_id", "kind"}}`. Tapping it opens that session.

## 8. Gateway HTTP additions (WP5)

All with `Authorization: Bearer ses_…`; `nzx_` keys are refused (403). Shapes follow
`docs/GATEWAY_CONTRACT.md` §3.3 for errors.

- `POST /api/push/devices {device_id, expo_push_token, platform, needs_me, finished}` →
  `{status: "registered"}`. `device_id` is the phone's relay device id; one row per account and
  device, replaced on every call. `expo_push_token` matches `ExponentPushToken[…]`.
- `DELETE /api/push/devices/:device_id` → `{status: "removed"}`.
- `GET /api/sessions` (A9) → `{sessions: [{id, client, device_name, os, created_at, last_used_at,
  current}]}`. The app hides the list when this answers 404.
- `DELETE /api/sessions/:id`, and `DELETE /api/sessions/current` for signing this phone out.
- `POST /auth/device/code` accepts `client_id: "neuron-mobile"`.
- The phone shows `GET /api/account`'s `user.email`, `plan.name`, `usage_today` and `limits`,
  and no purchase UI (store rules).

## 9. What each party must do

**Relay (WP5, A11 and A12).** Rooms per account; the frame rules in §4–§7; pairings in Durable
Object storage; limits (3 computers and 5 phones per account, 16 shares per computer, 1 MB frames,
a frame-rate cap); no plaintext anywhere, no frame bodies or tokens in logs; the push service and the
HTTP endpoints in §8. Port `dev/relay-room.ts` and run `test/relay.integration.test.ts` against it
(swap `startDevRelay` for the deployed URL and a real sign-in).

**Computer (WP1, R1 `neuron engine --share`).** Keep a host identity in the Neuron config (host id,
X25519 secret, presence key) and the paired phones (`phone_id`, name, `pair_key`). Connect out to
the relay with the user's session; `share` each shared session; offer a code and show the QR for
*Continue on phone* and `/phone`; answer `pair` (check the proof, `paired` with the sealed secrets);
accept channels from paired phones only; bridge each channel to the session's remote protocol 1
stream, letting through only `hello` (with the local token put in), `send`, `ping` and the read-only
`rpc` methods (`fs.list`, `fs.read`, `fs.search`, `git.status`, `git.diff`), never terminals or
restarts; send presence and `notify`. Match the vectors in §3.7. `src/relay/share-host.ts` is the
reference.

**Phone (WP4, this package).** Done: pairing, the reconnect and resume rules, presence, push
registration and the session over `RelayEngineTransport`.

## 10. Differences from `docs/NEURON_APPS_PLAN.md` §5

- **AES-256-GCM, not XChaCha20-Poly1305.** The parity plan (M2) chose AES-GCM. With fresh keys
  per channel and a per-direction counter as the nonce, GCM's 96-bit nonce never repeats.
- **One endpoint per device, not `/relay/host` and `/relay/client`.** The role is in `join`. One
  socket carries every pairing and channel of the device; the parity plan's `/v1/pair/:id` became
  the `pair_id` inside the `pair` frame.
- **The code never reaches the relay.** Only its hash does, so the relay cannot pair in the phone's
  place; the QR also carries the computer's key, so the relay cannot sit in the middle.
- **Tokens in a subprotocol**, since the room is chosen before the first frame (§4.1).
