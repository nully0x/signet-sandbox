// JSON-RPC 2.0 client over HTTP for the provisioning API.

use std::time::Duration;

use serde_json::Value;
use signet_rpc::envelope::{Id, Request, Response, Version};
use signet_rpc::error::Error as RpcError;

use crate::auth::Auth;

#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("{0}")]
    Rpc(RpcError),
    #[error("request to the API failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("invalid response from the API: {0}")]
    Protocol(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub struct Client {
    http: reqwest::blocking::Client,
    api_url: String,
    auth: Auth,
}

impl Client {
    pub fn new(api_url: &str, auth: Auth) -> anyhow::Result<Self> {
        Ok(Self {
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?,
            api_url: api_url.trim_end_matches('/').to_string(),
            auth,
        })
    }

    /// The exact URL requests go to; NIP-98 signatures must cover it.
    pub fn rpc_url(&self) -> String {
        format!("{}/v1/rpc", self.api_url)
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value, CallError> {
        let url = self.rpc_url();
        let authorization = self.auth.header(&url, "POST")?;
        let request = Request {
            jsonrpc: Version::V2,
            id: Some(Id::Number(1)),
            method: method.to_string(),
            params,
        };
        let response = self
            .http
            .post(&url)
            .header(reqwest::header::AUTHORIZATION, authorization)
            .json(&request)
            .send()?;
        let status = response.status();
        let body = response.bytes()?;
        let parsed: Response = serde_json::from_slice(&body).map_err(|e| {
            CallError::Protocol(format!("HTTP {status}: not a JSON-RPC response: {e}"))
        })?;
        split_response(parsed)
    }
}

fn split_response(response: Response) -> Result<Value, CallError> {
    match (response.result, response.error) {
        (Some(result), _) => Ok(result),
        (None, Some(error)) => Err(CallError::Rpc(error)),
        (None, None) => Err(CallError::Protocol(
            "response has neither result nor error".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// One-shot HTTP server returning a canned body; asserts the request
    /// carried the expected method and an Authorization header.
    fn serve(body: &'static str, expect_method: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(request.contains("authorization:"), "missing auth header");
            assert!(request.contains(expect_method), "wrong rpc method sent");
            let http = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(http.as_bytes()).unwrap();
        });
        format!("http://{addr}")
    }

    fn client(base: &str) -> Client {
        Client::new(base, Auth::Bearer("sgn_test".to_string())).unwrap()
    }

    #[test]
    fn call_returns_the_result_payload() {
        let base = serve(
            r#"{"jsonrpc":"2.0","id":1,"result":{"txid":"aa"}}"#,
            "environment.faucet",
        );
        let result = client(&base)
            .call("environment.faucet", serde_json::json!({"id": "e"}))
            .unwrap();
        assert_eq!(result["txid"], "aa");
    }

    #[test]
    fn call_maps_error_objects() {
        let base = serve(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32004,"message":"forbidden"}}"#,
            "environment.destroy",
        );
        let err = client(&base)
            .call("environment.destroy", serde_json::json!({"id": "e"}))
            .unwrap_err();
        match err {
            CallError::Rpc(e) => {
                assert_eq!(e.code, -32004);
                assert_eq!(e.message, "forbidden");
            }
            other => panic!("expected rpc error, got {other}"),
        }
    }

    #[test]
    fn call_rejects_non_json_rpc_bodies() {
        let base = serve("<html>502</html>", "environment.get");
        let err = client(&base)
            .call("environment.get", serde_json::json!({"id": "e"}))
            .unwrap_err();
        assert!(matches!(err, CallError::Protocol(_)));
    }
}
