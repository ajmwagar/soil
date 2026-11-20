//! Host-owned WSS ingress for `soil:stream`.
//!
//! TLS terminates at the proxy in front of this listener. The host authenticates
//! the query token, turns the WebSocket into a bounded duplex session, and puts
//! it on the stream hub. A Spore only sees bytes and peer metadata.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use soil::{StreamHub, StreamInfo, StreamSession};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

pub fn spawn(hub: StreamHub, listen: SocketAddr, token: Arc<str>) -> Result<()> {
    std::thread::Builder::new()
        .name("soil-wss".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("soil: wss runtime: {error}");
                    return;
                }
            };
            if let Err(error) = runtime.block_on(serve(hub, listen, token)) {
                eprintln!("soil: wss: {error:#}");
            }
        })
        .context("spawn wss listener")?;
    Ok(())
}

async fn serve(hub: StreamHub, listen: SocketAddr, token: Arc<str>) -> Result<()> {
    let listener = TcpListener::bind(listen)
        .await
        .with_context(|| format!("bind wss {listen}"))?;
    println!("soil: wss on ws://{listen}/stream");
    loop {
        let (socket, peer) = listener.accept().await.context("accept wss")?;
        let hub = hub.clone();
        let token = token.clone();
        tokio::spawn(async move {
            if let Err(error) = handle(socket, peer, hub, token).await {
                eprintln!("soil: wss {peer}: {error:#}");
            }
        });
    }
}

async fn handle(
    socket: tokio::net::TcpStream,
    peer: SocketAddr,
    hub: StreamHub,
    token: Arc<str>,
) -> Result<()> {
    let socket = tokio_tungstenite::accept_hdr_async(socket, |request: &Request, response: Response| {
        if authorized(request, &token) {
            Ok(response)
        } else {
            let body = Some("unauthorized".to_string());
            Err(http::Response::builder().status(401).body(body).unwrap())
        }
    })
    .await
    .context("websocket handshake")?;
    let (mut sink, mut source) = socket.split();
    let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(32);
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(32);
    hub.attach(StreamSession::new(
        StreamInfo {
            protocol: "wss".into(),
            peer: Some(peer.to_string()),
            principal: None,
            media_type: Some("application/octet-stream".into()),
        },
        inbound_rx,
        outbound_tx,
    ));

    loop {
        tokio::select! {
            message = source.next() => {
                match message {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(bytes))) => {
                        if inbound_tx.send(bytes.to_vec()).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                        if inbound_tx.send(text.as_bytes().to_vec()).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => break,
                    Some(Err(error)) => return Err(error.into()),
                    _ => {}
                }
            }
            outgoing = outbound_rx.recv() => {
                match outgoing {
                    Some(bytes) => sink
                        .send(tokio_tungstenite::tungstenite::Message::Binary(bytes.into()))
                        .await
                        .context("wss write")?,
                    None => break,
                }
            }
        }
    }
    Ok(())
}

fn authorized(request: &Request, token: &str) -> bool {
    if token.is_empty() {
        return true;
    }
    let query = request.uri().query().unwrap_or_default();
    query.split('&').any(|pair| {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        name == "token" && value == token
    })
}
