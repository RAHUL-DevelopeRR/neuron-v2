//! `run()` over real sockets: this test is the relay (and the phone behind it) and the session's
//! local host, and checks what the share sends each of them: the session in the Authorization
//! header and the relay subprotocol, `join`, a pairing with proofs, sealed presence, `notify`, an
//! encrypted channel whose `hello` reaches the local host with the host's peer token (never its
//! owner's), a refused terminal, and `unshare` on the way out. A second test checks that the share
//! stops, and shares nothing, when the host does not call its token a peer's.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use neuron_share::contract::Role;
use neuron_share::crypto::{
    accept_confirm, b64u, bytes_of, derive_channel_keys, derive_pair_key, key_pair, open_blob,
    open_frame, pair_proof, pair_secrets_aad, presence_aad, seal_frame, subkey, Direction,
};
use neuron_share::pairing::PairingPayload;
use neuron_share::{Reporter, ShareOptions};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// The local host's peer token: the only credential the share holds.
const PEER_TOKEN: &str = "peer-token-secret";
/// The local host's owner token: a share given this one by mistake must refuse to run.
const OWNER_TOKEN: &str = "owner-token-secret";
const SESSION: &str = "ses_test_session_token";

enum Said {
    Offer(PairingPayload),
    Paired(String),
}

struct TestReporter(mpsc::UnboundedSender<Said>);

impl Reporter for TestReporter {
    fn status(&mut self, _line: &str) {}
    fn offer(&mut self, payload: &PairingPayload) {
        let _ = self.0.send(Said::Offer(payload.clone()));
    }
    fn paired(&mut self, phone_name: &str) {
        let _ = self.0.send(Said::Paired(phone_name.to_string()));
    }
}

async fn next_json(socket: &mut WebSocketStream<TcpStream>) -> Value {
    loop {
        let message = timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("a frame in time")
            .expect("the socket is open")
            .expect("a frame");
        if let Message::Text(text) = message {
            return serde_json::from_str(text.as_str()).unwrap();
        }
    }
}

/// Reads frames until one has type `t`, returning it and the ones before.
async fn until(socket: &mut WebSocketStream<TcpStream>, t: &str) -> (Value, Vec<Value>) {
    let mut before = Vec::new();
    loop {
        let frame = next_json(socket).await;
        if frame["t"] == t {
            return (frame, before);
        }
        before.push(frame);
    }
}

async fn say(socket: &mut WebSocketStream<TcpStream>, frame: Value) {
    socket.send(Message::text(frame.to_string())).await.unwrap();
}

/// The session's local host: answers each client's `hello` with `welcome` and a `snapshot`,
/// records what bridge clients send, and finishes the turn for the observer on cue.
async fn local_host(
    listener: TcpListener,
    seen: Arc<Mutex<Vec<Value>>>,
    finish: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let seen = Arc::clone(&seen);
        let finish = Arc::clone(&finish);
        tokio::spawn(async move {
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let hello = next_json(&mut socket).await;
            assert_eq!(hello["t"], "hello");
            assert_eq!(
                hello["token"], PEER_TOKEN,
                "every client brings the peer token, never the owner's"
            );
            let observer = hello["client"]["kind"] == "cli";
            // What the Neuron host answers its peer token with.
            say(&mut socket, json!({ "t": "welcome", "protocol": 1, "host": { "name": "Dev", "version": null, "cwd": "/work/demo" }, "clients": 1, "role": "peer" })).await;
            say(&mut socket, json!({ "t": "snapshot", "seq": 3, "state": {
                "connection": "ready", "busy": true, "sessionStarting": false,
                "status": { "cwd": "/work/demo", "model": "glm-5.2", "session_id": "s1", "git": { "branch": "main" } },
                "sessions": null, "items": [{ "kind": "user", "key": "i1", "text": "Fix it", "attachments": [] }],
                "permissions": [], "nextKey": 2 } })).await;
            if observer {
                let cue = finish.lock().unwrap().take();
                if let Some(cue) = cue {
                    let _ = cue.await;
                    say(
                        &mut socket,
                        json!({ "t": "action", "seq": 4, "action": { "type": "turn_finished" } }),
                    )
                    .await;
                }
                // Keep the observer connected.
                while socket.next().await.is_some() {}
                return;
            }
            seen.lock().unwrap().push(hello);
            while let Some(Ok(Message::Text(text))) = socket.next().await {
                let frame: Value = serde_json::from_str(text.as_str()).unwrap();
                if frame["t"] == "ping" {
                    say(&mut socket, json!({ "t": "pong" })).await;
                }
                seen.lock().unwrap().push(frame);
            }
        });
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines, clippy::result_large_err)] // one flow, in order; the handshake callback's type is tungstenite's
async fn shares_pairs_and_bridges_over_sockets() {
    let home = std::env::temp_dir().join(format!(
        "neuron-share-flow-{}",
        neuron_share::crypto::random_id()
    ));
    let local = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_url = format!("ws://{}/neuron", local.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (finish_tx, finish_rx) = oneshot::channel();
    tokio::spawn(local_host(
        local,
        Arc::clone(&seen),
        Arc::new(Mutex::new(Some(finish_rx))),
    ));

    let relay = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_url = format!("ws://{}/v1/relay", relay.local_addr().unwrap());
    let (said_tx, mut said) = mpsc::unbounded_channel();
    let (stop_tx, stop_rx) = oneshot::channel();
    let share = tokio::spawn(neuron_share::run(
        ShareOptions {
            relay_url: relay_url.clone(),
            token: SESSION.into(),
            config_dir: home.clone(),
            name: "Dev laptop".into(),
            platform: "test".into(),
            local_url,
            peer_token: PEER_TOKEN.into(),
            pair_ttl_s: 600,
            offer_at_start: true,
            read_stdin: false,
            stop: Some(stop_rx),
        },
        Box::new(TestReporter(said_tx)),
    ));

    // The relay's side of the handshake: the session in the header, the subprotocol echoed.
    let (stream, _) = relay.accept().await.unwrap();
    let headers = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&headers);
    let mut socket = tokio_tungstenite::accept_hdr_async(
        stream,
        move |request: &Request, mut response: Response| {
            *captured.lock().unwrap() = Some((
                request.headers().get("authorization").cloned(),
                request.headers().get("sec-websocket-protocol").cloned(),
            ));
            response.headers_mut().insert(
                "Sec-WebSocket-Protocol",
                HeaderValue::from_static("neuron-relay.v1"),
            );
            Ok(response)
        },
    )
    .await
    .unwrap();
    let (authorization, protocol) = headers.lock().unwrap().take().unwrap();
    assert_eq!(authorization.unwrap(), format!("Bearer {SESSION}").as_str());
    assert_eq!(
        protocol.unwrap(),
        "neuron-relay.v1",
        "the token is not in the subprotocol"
    );
    let join = next_json(&mut socket).await;
    assert_eq!(join["t"], "join");
    assert_eq!(join["role"], "host");
    assert_eq!(join["device"]["name"], "Dev laptop");
    let host_id = join["device"]["id"].as_str().unwrap().to_string();
    say(&mut socket, json!({ "t": "joined", "relay": 1, "role": "host", "device_id": host_id, "account": { "tag": "0123456789abcdef" } })).await;

    // Pairing: the code comes from the QR (the reporter), never from the relay.
    let (offer, _) = until(&mut socket, "pair_offer").await;
    let pair_id = offer["pair_id"].as_str().unwrap().to_string();
    say(
        &mut socket,
        json!({ "t": "pair_offer_ok", "pair_id": pair_id, "expires_at": 4_102_444_800_000_u64 }),
    )
    .await;
    let Some(Said::Offer(payload)) = said.recv().await else {
        panic!("no offer shown")
    };
    assert_eq!(payload.relay_url, relay_url);
    assert_eq!(payload.account_tag.as_deref(), Some("0123456789abcdef"));
    assert!(
        !offer.to_string().contains(&payload.code),
        "the code never reaches the relay"
    );
    let phone = key_pair();
    let host_key = bytes_of::<32>(&payload.host_key, "key").unwrap();
    let pair_key = derive_pair_key(
        &phone.secret,
        &host_key,
        &payload.code,
        &host_key,
        &phone.public,
    )
    .unwrap();
    say(&mut socket, json!({ "t": "pair", "pair_id": pair_id, "phone_id": "phone-0001", "pub": b64u(&phone.public), "proof": pair_proof(&pair_key, Role::Phone, &pair_id), "name": "Pixel" })).await;
    let (paired, _) = until(&mut socket, "paired").await;
    assert_eq!(paired["proof"], pair_proof(&pair_key, Role::Host, &pair_id));
    let secrets: Value = serde_json::from_str(
        &open_blob(
            &subkey(&pair_key, "pair-secrets"),
            &pair_secrets_aad(&pair_id),
            paired["sealed"].as_str().unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let presence_key = bytes_of::<32>(secrets["presence_key"].as_str().unwrap(), "key").unwrap();
    assert!(matches!(said.recv().await, Some(Said::Paired(name)) if name == "Pixel"));

    // Presence: sealed, from the local host's session; then the turn ends and the relay hears it.
    let (share_frame, _) = until(&mut socket, "share").await;
    let share_id = share_frame["share_id"].as_str().unwrap().to_string();
    let presence: Value = serde_json::from_str(
        &open_blob(
            &presence_key,
            &presence_aad(&host_id, &share_id),
            share_frame["sealed"].as_str().unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (
            presence["project"].as_str(),
            presence["status"].as_str(),
            presence["title"].as_str()
        ),
        (Some("demo"), Some("running"), Some("Fix it"))
    );
    finish_tx.send(()).unwrap();
    let (notify, before) = until(&mut socket, "notify").await;
    assert_eq!(
        notify,
        json!({ "t": "notify", "share_id": share_id, "kind": "finished" })
    );
    assert!(
        before.iter().any(|frame| frame["t"] == "share"),
        "the idle presence went first"
    );

    // A channel: accept with a confirm, then remote protocol 1 inside, sealed both ways.
    let ch = "channel-0000000001";
    let ephemeral = key_pair();
    say(&mut socket, json!({ "t": "open", "ch": ch, "phone_id": "phone-0001", "share_id": share_id, "e": b64u(&ephemeral.public) })).await;
    let (accept, _) = until(&mut socket, "accept").await;
    let host_ephemeral = bytes_of::<32>(accept["e"].as_str().unwrap(), "e").unwrap();
    let keys = derive_channel_keys(
        &ephemeral.secret,
        &host_ephemeral,
        &pair_key,
        &ephemeral.public,
        &host_ephemeral,
        ch,
    )
    .unwrap();
    assert_eq!(accept["confirm"], accept_confirm(&keys, ch));
    for (n, inner) in [
        json!({ "t": "hello", "protocol": 1, "token": "", "client": { "name": "Pixel", "kind": "mobile" } }),
        json!({ "t": "rpc", "id": 1, "method": "term.open", "params": {} }),
        json!({ "t": "ping" }),
    ]
    .into_iter()
    .enumerate()
    {
        let n = u64::try_from(n).unwrap();
        say(&mut socket, json!({ "t": "data", "ch": ch, "n": n, "c": seal_frame(&keys.p2h, ch, Direction::P2h, n, &inner.to_string()) })).await;
    }
    let mut opened = Vec::new();
    while opened.len() < 4 {
        let (data, _) = until(&mut socket, "data").await;
        let text = open_frame(
            &keys.h2p,
            ch,
            Direction::H2p,
            data["n"].as_u64().unwrap(),
            data["c"].as_str().unwrap(),
        )
        .unwrap();
        opened.push(
            serde_json::from_str::<Value>(&text).unwrap()["t"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    opened.sort();
    assert_eq!(
        opened,
        ["pong", "rpc_result", "snapshot", "welcome"],
        "the host's frames and the refusal come back sealed"
    );
    let local_frames = seen.lock().unwrap().clone();
    assert_eq!(
        local_frames[0]["client"]["kind"], "mobile",
        "the phone's hello reached the host with the token put in"
    );
    assert!(
        local_frames.iter().all(|frame| frame["t"] != "rpc"),
        "the terminal never reached the host"
    );

    // Stopping: the session is unshared and the channel closed.
    stop_tx.send(()).unwrap();
    let mut last = Vec::new();
    while let Ok(Some(Ok(Message::Text(text)))) =
        timeout(Duration::from_secs(5), socket.next()).await
    {
        last.push(serde_json::from_str::<Value>(text.as_str()).unwrap());
        if last.iter().any(|frame| frame["t"] == "unshare") {
            break;
        }
    }
    assert!(
        last.contains(&json!({ "t": "close", "ch": ch })),
        "{last:?}"
    );
    assert!(last.contains(&json!({ "t": "unshare", "share_id": share_id })));
    timeout(Duration::from_secs(5), share)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("remote.json")).unwrap()).unwrap();
    assert_eq!(
        saved["phones"][0]["phone_id"], "phone-0001",
        "the pairing is saved"
    );
    assert_eq!(
        saved["shares"]["/work/demo"],
        share_id.as_str(),
        "the share id is kept for the project"
    );
    std::fs::remove_dir_all(home).unwrap();
}

/// A local host that answers each client's `hello` with a `welcome` that says `role`, or says nothing
/// about one, as a host older than the credential check does. The token the share holds is
/// `OWNER_TOKEN`: the mistake to catch.
async fn host_saying(listener: TcpListener, role: Option<&'static str>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let hello = next_json(&mut socket).await;
            assert_eq!(hello["t"], "hello");
            assert_eq!(hello["token"], OWNER_TOKEN);
            let mut welcome = json!({ "t": "welcome", "protocol": 1, "host": { "name": "Dev", "version": null, "cwd": "/work/demo" }, "clients": 1 });
            if let Some(role) = role {
                welcome["role"] = json!(role);
            }
            say(&mut socket, welcome).await;
            while socket.next().await.is_some() {}
        });
    }
}

/// The relay's side of a share's handshake: accepts the socket and echoes the relay subprotocol.
#[allow(clippy::result_large_err)] // the handshake callback's type is tungstenite's
async fn accept_relay(stream: TcpStream) -> WebSocketStream<TcpStream> {
    tokio_tungstenite::accept_hdr_async(stream, |_: &Request, mut response: Response| {
        response.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_static("neuron-relay.v1"),
        );
        Ok(response)
    })
    .await
    .unwrap()
}

/// Runs a share that holds `OWNER_TOKEN` against a local host whose `welcome` says `role`, with a relay
/// that welcomes it. Returns why the share stopped and what the relay heard after `joined`.
async fn share_that_is_refused(role: Option<&'static str>) -> (String, Vec<Value>) {
    let home = std::env::temp_dir().join(format!(
        "neuron-share-role-{}",
        neuron_share::crypto::random_id()
    ));
    let local = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_url = format!("ws://{}/neuron", local.local_addr().unwrap());
    tokio::spawn(host_saying(local, role));
    let relay = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_url = format!("ws://{}/v1/relay", relay.local_addr().unwrap());
    let (said_tx, _said) = mpsc::unbounded_channel();
    let share = tokio::spawn(neuron_share::run(
        ShareOptions {
            relay_url,
            token: SESSION.into(),
            config_dir: home.clone(),
            name: "Dev laptop".into(),
            platform: "test".into(),
            local_url,
            peer_token: OWNER_TOKEN.into(),
            pair_ttl_s: 600,
            offer_at_start: false,
            read_stdin: false,
            stop: None,
        },
        Box::new(TestReporter(said_tx)),
    ));
    let (stream, _) = relay.accept().await.unwrap();
    let mut socket = accept_relay(stream).await;
    let join = next_json(&mut socket).await;
    assert_eq!(join["t"], "join");
    let host_id = join["device"]["id"].as_str().unwrap().to_string();
    say(
        &mut socket,
        json!({ "t": "joined", "relay": 1, "role": "host", "device_id": host_id }),
    )
    .await;
    let error = timeout(Duration::from_secs(10), share)
        .await
        .expect("the share stops")
        .unwrap()
        .expect_err("a share the host does not call a peer's is an error");
    let mut heard = Vec::new();
    while let Ok(Some(Ok(Message::Text(text)))) =
        timeout(Duration::from_millis(300), socket.next()).await
    {
        heard.push(serde_json::from_str::<Value>(text.as_str()).unwrap());
    }
    let _ = std::fs::remove_dir_all(home);
    (error.to_string(), heard)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refuses_to_share_unless_the_host_calls_the_token_a_peers() {
    // The owner's token by mistake, a role nobody knows, and a host that predates roles.
    for role in [Some("owner"), Some("admin"), None] {
        let (message, heard) = share_that_is_refused(role).await;
        assert!(message.contains("peer token"), "{role:?}: {message}");
        assert!(
            heard.iter().all(|frame| frame["t"] != "share"),
            "{role:?}: a refused share shares nothing, the relay heard {heard:?}"
        );
    }
}
