//! WebSockets through the HTTP proxy a cloud VM names in `HTTPS_PROXY`.
//! A cloud VM reaches the internet only through that proxy, and tungstenite
//! speaks no proxy, so the socket opens a CONNECT tunnel itself and runs TLS
//! over it against the system's roots, which hold the proxy's CA. ureq
//! already takes the proxy from the environment.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

use tungstenite::WebSocket;
use tungstenite::client::IntoClientRequest;
use tungstenite::error::UrlError;
use tungstenite::handshake::client::Response;
use tungstenite::stream::MaybeTlsStream;

pub type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

/// `HTTPS_PROXY`, else `https_proxy`, when set.
pub fn from_env() -> Option<String> {
    let var = |name| std::env::var(name).ok().filter(|v: &String| !v.is_empty());
    var("HTTPS_PROXY").or_else(|| var("https_proxy"))
}

/// Opens the WebSocket `request` names, through `proxy` (`http://host:port`)
/// when there is one. A tunnel the proxy will not open is `UnableToConnect`,
/// as a host that cannot be reached is without one.
pub fn connect(
    request: impl IntoClientRequest,
    proxy: Option<&str>,
) -> tungstenite::Result<(Socket, Response)> {
    let Some(proxy) = proxy else {
        return tungstenite::connect(request);
    };
    let request = request.into_client_request()?;
    let unable = || tungstenite::Error::Url(UrlError::UnableToConnect(String::new()));
    let uri = request.uri();
    let host = uri.host().ok_or_else(unable)?;
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("wss") {
            443
        } else {
            80
        });
    let target = format!("{host}:{port}");
    let proxy_at = proxy
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    let mut stream = TcpStream::connect(&proxy_at).map_err(|_| unable())?;
    write!(
        stream,
        "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n"
    )?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut status = String::new();
    reader.read_line(&mut status)?;
    if status.split_whitespace().nth(1) != Some("200") {
        return Err(unable());
    }
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
    }
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .and_then(|b| {
            use rustls_platform_verifier::BuilderVerifierExt;
            b.with_platform_verifier()
        })
        .map_err(|e| tungstenite::Error::Tls(e.into()))?
        .with_no_client_auth();
    let connector = tungstenite::Connector::Rustls(std::sync::Arc::new(config));
    tungstenite::client_tls_with_config(request, stream, None, Some(connector)).map_err(|e| match e
    {
        tungstenite::HandshakeError::Failure(e) => e,
        tungstenite::HandshakeError::Interrupted(_) => unable(),
    })
}
