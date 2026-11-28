// SPDX-License-Identifier: AGPL-3.0-or-later

//! HTTPS client for the self-hosted sync service. TLS uses the operating
//! system (SChannel on Windows, Security.framework on macOS), so certificates
//! from a private CA trusted by the system are accepted.

use reqwest::{Client, Method, StatusCode, redirect};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::time::Duration;
use url::{Host, Url};
use zhiye_core::{Error, Result};
use zhiye_sync_protocol::{
    BlobQuery, DeviceDraft, ErrorBody, HealthInfo, IssuedDevice, MAX_BLOB_QUERY, MissingBlobs,
    PROTOCOL_VERSION, PullResponse, PushRequest, PushResponse, WorkspaceDraft, WorkspaceInfo,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Originals can be up to 100 MiB; allow slow links.
const BLOB_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Validates and normalizes a service address. Plain HTTP is accepted only
/// for this computer; everything else must use HTTPS.
pub fn server_url(input: &str) -> Result<Url> {
    let invalid = || {
        Error::new(
            "SYNC_SERVER_URL",
            "请输入同步服务地址，例如 https://sync.example.com。",
        )
    };
    let mut url = Url::parse(input.trim()).map_err(|_| invalid())?;
    if url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    match url.scheme() {
        "https" => {}
        "http" if is_loopback(&url) => {}
        "http" => {
            return Err(Error::new(
                "SYNC_INSECURE",
                "同步服务必须使用 https:// 地址。只有本机测试服务可以使用 http://。",
            ));
        }
        _ => return Err(invalid()),
    }
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&format!("{path}/"));
    Ok(url)
}

/// The form stored and shown to the user, without the trailing slash.
pub fn display(url: &Url) -> String {
    url.as_str().trim_end_matches('/').to_owned()
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

pub struct Remote {
    client: Client,
    base: Url,
    token: Option<String>,
}

impl Remote {
    pub fn new(base: Url, token: Option<String>) -> Result<Self> {
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            // A redirect could carry the bearer token to another host.
            .redirect(redirect::Policy::none())
            .https_only(base.scheme() == "https")
            .user_agent(concat!("zhiye-desktop/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| network_error(&error))?;
        Ok(Self {
            client,
            base,
            token,
        })
    }

    pub async fn health(&self) -> Result<HealthInfo> {
        let health: HealthInfo = self
            .send(Method::GET, "v1/health", None, None::<&()>)
            .await?;
        if health.protocol != PROTOCOL_VERSION {
            return Err(Error::new(
                "SYNC_PROTOCOL",
                format!(
                    "同步服务使用协议 {}，本机支持协议 {PROTOCOL_VERSION}。请让两者使用匹配的版本。",
                    health.protocol
                ),
            ));
        }
        Ok(health)
    }

    pub async fn register(&self, admin_token: &str, name: &str) -> Result<IssuedDevice> {
        self.send(
            Method::POST,
            "v1/devices",
            Some(admin_token),
            Some(&DeviceDraft { name: name.into() }),
        )
        .await
    }

    pub async fn workspaces(&self) -> Result<Vec<WorkspaceInfo>> {
        self.device(Method::GET, "v1/workspaces".into(), None::<&()>)
            .await
    }

    pub async fn put_workspace(&self, id: &str, name: &str) -> Result<WorkspaceInfo> {
        self.device(
            Method::PUT,
            format!("v1/workspaces/{id}"),
            Some(&WorkspaceDraft { name: name.into() }),
        )
        .await
    }

    pub async fn push(&self, workspace: &str, request: &PushRequest) -> Result<PushResponse> {
        self.device(
            Method::POST,
            format!("v1/workspaces/{workspace}/push"),
            Some(request),
        )
        .await
    }

    pub async fn pull(&self, workspace: &str, since: u64) -> Result<PullResponse> {
        self.device(
            Method::GET,
            format!("v1/workspaces/{workspace}/changes?since={since}"),
            None::<&()>,
        )
        .await
    }

    /// Hashes among `hashes` that the service does not hold yet.
    pub async fn missing_blobs(&self, workspace: &str, hashes: &[String]) -> Result<Vec<String>> {
        let mut missing = Vec::new();
        for chunk in hashes.chunks(MAX_BLOB_QUERY) {
            let answer: MissingBlobs = self
                .device(
                    Method::POST,
                    format!("v1/workspaces/{workspace}/blobs/missing"),
                    Some(&BlobQuery {
                        hashes: chunk.to_vec(),
                    }),
                )
                .await?;
            missing.extend(answer.missing);
        }
        Ok(missing)
    }

    pub async fn upload_blob(&self, workspace: &str, hash: &str, bytes: Vec<u8>) -> Result<()> {
        let response = self
            .blob_request(Method::PUT, workspace, hash)?
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes)
            .send()
            .await
            .map_err(|error| network_error(&error))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.bytes().await.unwrap_or_default();
        Err(service_error(status, &body))
    }

    pub async fn download_blob(&self, workspace: &str, hash: &str) -> Result<Vec<u8>> {
        let response = self
            .blob_request(Method::GET, workspace, hash)?
            .send()
            .await
            .map_err(|error| network_error(&error))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|error| network_error(&error))?;
        if status.is_success() {
            return Ok(body.to_vec());
        }
        Err(service_error(status, &body))
    }

    fn blob_request(
        &self,
        method: Method,
        workspace: &str,
        hash: &str,
    ) -> Result<reqwest::RequestBuilder> {
        let token = self
            .token
            .as_deref()
            .ok_or_else(|| Error::new("SYNC_DEVICE", "请先登记本机设备。"))?;
        let url = self
            .base
            .join(&format!("v1/workspaces/{workspace}/blobs/{hash}"))
            .map_err(|_| Error::new("SYNC_SERVER_URL", "无法组成同步服务的请求地址。"))?;
        Ok(self
            .client
            .request(method, url)
            .bearer_auth(token)
            .timeout(BLOB_TIMEOUT))
    }

    async fn device<T: DeserializeOwned>(
        &self,
        method: Method,
        path: String,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        let token = self
            .token
            .as_deref()
            .ok_or_else(|| Error::new("SYNC_DEVICE", "请先登记本机设备。"))?;
        self.send(method, &path, Some(token), body).await
    }

    async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        let url = self
            .base
            .join(path)
            .map_err(|_| Error::new("SYNC_SERVER_URL", "无法组成同步服务的请求地址。"))?;
        let mut request = self.client.request(method, url);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(serde_json::to_vec(body)?);
        }
        let response = request
            .send()
            .await
            .map_err(|error| network_error(&error))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| network_error(&error))?;
        if status.is_success() {
            return serde_json::from_slice(&bytes).map_err(|_| {
                Error::new(
                    "SYNC_RESPONSE",
                    "同步服务的响应格式无法识别。请确认地址指向知页同步服务。",
                )
            });
        }
        Err(service_error(status, &bytes))
    }
}

fn service_error(status: StatusCode, body: &[u8]) -> Error {
    let parsed = serde_json::from_slice::<ErrorBody>(body).ok();
    let message = parsed
        .as_ref()
        .map(|body| body.message.clone())
        .unwrap_or_else(|| format!("同步服务返回 HTTP {}。", status.as_u16()));
    let code = match parsed.as_ref().map(|body| body.code.as_str()) {
        Some("UNAUTHORIZED") => "SYNC_UNAUTHORIZED",
        Some("WORKSPACE_NOT_FOUND") => "SYNC_WORKSPACE_NOT_FOUND",
        Some("OPERATION_REUSED") => "SYNC_OPERATION_REUSED",
        Some("BLOB_MISSING") => "SYNC_BLOB_MISSING",
        Some("BLOB_NOT_FOUND") => "SYNC_BLOB_NOT_FOUND",
        _ if status.is_redirection() => {
            return Error::new(
                "SYNC_REDIRECT",
                "同步服务返回了重定向。请填写最终的 https:// 地址。",
            );
        }
        _ => "SYNC_SERVER",
    };
    Error::new(code, message)
}

fn network_error(error: &reqwest::Error) -> Error {
    // Include the underlying cause, such as a certificate or DNS failure.
    let mut detail = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        let text = cause.to_string();
        if !detail.contains(&text) {
            detail.push('：');
            detail.push_str(&text);
        }
        source = cause.source();
    }
    let message = if error.is_timeout() {
        format!("连接同步服务超时。{detail}")
    } else {
        format!("无法连接同步服务。请检查地址、网络和证书。{detail}")
    };
    Error::new("SYNC_NETWORK", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https_and_loopback_http_only() {
        let url = server_url(" https://sync.example.com/zhiye/ ").unwrap();
        assert_eq!(url.as_str(), "https://sync.example.com/zhiye/");
        assert_eq!(display(&url), "https://sync.example.com/zhiye");
        assert_eq!(
            server_url("https://sync.example.com")
                .unwrap()
                .join("v1/health")
                .unwrap()
                .as_str(),
            "https://sync.example.com/v1/health"
        );
        for local in [
            "http://127.0.0.1:8787",
            "http://localhost:8787",
            "http://[::1]:8787",
        ] {
            assert!(server_url(local).is_ok(), "{local}");
        }
        assert_eq!(
            server_url("http://192.168.1.20:8787").unwrap_err().code,
            "SYNC_INSECURE"
        );
        for invalid in [
            "",
            "sync.example.com",
            "ftp://sync.example.com",
            "https://user:pass@sync.example.com",
            "https://sync.example.com/?token=1",
            "https://sync.example.com/#x",
        ] {
            assert_eq!(
                server_url(invalid).unwrap_err().code,
                "SYNC_SERVER_URL",
                "{invalid}"
            );
        }
    }

    #[test]
    fn maps_service_errors() {
        let body = r#"{"code":"UNAUTHORIZED","message":"设备凭据无效或已被撤销。"}"#;
        let error = service_error(StatusCode::UNAUTHORIZED, body.as_bytes());
        assert_eq!(error.code, "SYNC_UNAUTHORIZED");
        assert_eq!(error.message, "设备凭据无效或已被撤销。");
        let error = service_error(StatusCode::BAD_GATEWAY, b"<html>");
        assert_eq!(error.code, "SYNC_SERVER");
        assert!(error.message.contains("502"));
        assert_eq!(
            service_error(StatusCode::MOVED_PERMANENTLY, b"").code,
            "SYNC_REDIRECT"
        );
    }
}
