// SPDX-License-Identifier: AGPL-3.0-or-later

//! HTTP/1.1 front end. TLS is terminated by a reverse proxy in front of the
//! service; the binary only listens on loopback unless told otherwise.

use crate::engine::{Device, Engine, ServiceError, ServiceResult, digest};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio::time::timeout;
use zhiye_sync_protocol::{
    BlobQuery, DeviceDraft, ErrorBody, HealthInfo, MAX_BLOB_BYTES, MAX_REQUEST_BYTES,
    PROTOCOL_VERSION, PushRequest, WorkspaceDraft,
};

const MAX_CONNECTIONS: usize = 256;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(300);

type Reply = Response<Full<Bytes>>;

pub struct Service {
    engine: Engine,
    admin_digest: Vec<u8>,
}

impl Service {
    /// `admin_token` authorizes device registration and revocation.
    pub fn new(engine: Engine, admin_token: &str) -> ServiceResult<Self> {
        if admin_token.chars().count() < 32 || admin_token.chars().any(char::is_whitespace) {
            return Err(ServiceError::new(
                500,
                "ADMIN_TOKEN",
                "管理令牌至少需要 32 个字符，且不能包含空白。",
            ));
        }
        Ok(Self {
            engine,
            admin_digest: digest(admin_token.as_bytes()),
        })
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }
}

/// Accepts connections until the listener fails.
pub async fn serve(listener: TcpListener, service: Arc<Service>) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            _ = connections.join_next(), if !connections.is_empty() => {}
            accepted = listener.accept(), if connections.len() < MAX_CONNECTIONS => {
                let Ok((socket, _)) = accepted else { return };
                let service = Arc::clone(&service);
                connections.spawn(async move {
                    let handler = service_fn(move |request| {
                        let service = Arc::clone(&service);
                        async move {
                            let reply = match dispatch(&service, request).await {
                                Ok(reply) => reply,
                                Err(error) => error_reply(&error),
                            };
                            Ok::<_, Infallible>(reply)
                        }
                    });
                    let mut http = http1::Builder::new();
                    http.timer(TokioTimer::new())
                        .header_read_timeout(REQUEST_TIMEOUT)
                        .max_buf_size(64 * 1024)
                        .max_headers(64);
                    let _ = timeout(
                        CONNECTION_TIMEOUT,
                        http.serve_connection(TokioIo::new(socket), handler),
                    )
                    .await;
                });
            }
        }
    }
}

async fn dispatch(service: &Arc<Service>, request: Request<Incoming>) -> ServiceResult<Reply> {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path().to_owned();
    let segments: Vec<&str> = path
        .strip_prefix("/v1/")
        .map(|rest| rest.split('/').collect())
        .ok_or_else(not_found)?;
    let method = parts.method.clone();
    match (method, segments.as_slice()) {
        (Method::GET, ["health"]) => json(
            StatusCode::OK,
            &HealthInfo {
                protocol: PROTOCOL_VERSION,
                service: "zhiye-sync".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
        ),
        (Method::GET, ["devices"]) => {
            admin(service, &parts.headers)?;
            let engine = Arc::clone(service);
            json(
                StatusCode::OK,
                &blocking(move || engine.engine.devices()).await?,
            )
        }
        (Method::POST, ["devices"]) => {
            admin(service, &parts.headers)?;
            let draft: DeviceDraft = read_json(&parts.headers, body).await?;
            let engine = Arc::clone(service);
            let issued = blocking(move || engine.engine.register_device(&draft.name)).await?;
            json(StatusCode::CREATED, &issued)
        }
        (Method::DELETE, ["devices", id]) => {
            admin(service, &parts.headers)?;
            let id = (*id).to_owned();
            let engine = Arc::clone(service);
            blocking(move || engine.engine.revoke_device(&id)).await?;
            Ok(empty(StatusCode::NO_CONTENT))
        }
        (Method::GET, ["workspaces"]) => {
            let device = device(service, &parts.headers).await?;
            let engine = Arc::clone(service);
            json(
                StatusCode::OK,
                &blocking(move || engine.engine.workspaces(&device)).await?,
            )
        }
        (Method::PUT, ["workspaces", id]) => {
            let device = device(service, &parts.headers).await?;
            let draft: WorkspaceDraft = read_json(&parts.headers, body).await?;
            let id = (*id).to_owned();
            let engine = Arc::clone(service);
            json(
                StatusCode::OK,
                &blocking(move || engine.engine.put_workspace(&device, &id, &draft.name)).await?,
            )
        }
        (Method::POST, ["workspaces", id, "push"]) => {
            let device = device(service, &parts.headers).await?;
            let request: PushRequest = read_json(&parts.headers, body).await?;
            let id = (*id).to_owned();
            let engine = Arc::clone(service);
            json(
                StatusCode::OK,
                &blocking(move || engine.engine.push(&device, &id, &request)).await?,
            )
        }
        (Method::POST, ["workspaces", id, "blobs", "missing"]) => {
            let device = device(service, &parts.headers).await?;
            let query: BlobQuery = read_json(&parts.headers, body).await?;
            let id = (*id).to_owned();
            let engine = Arc::clone(service);
            json(
                StatusCode::OK,
                &blocking(move || engine.engine.missing_blobs(&device, &id, &query)).await?,
            )
        }
        (Method::PUT, ["workspaces", id, "blobs", hash]) => {
            let device = device(service, &parts.headers).await?;
            let bytes = read_bytes(&parts.headers, body).await?;
            let (id, hash) = ((*id).to_owned(), (*hash).to_owned());
            let engine = Arc::clone(service);
            blocking(move || engine.engine.put_blob(&device, &id, &hash, &bytes)).await?;
            Ok(empty(StatusCode::NO_CONTENT))
        }
        (Method::GET, ["workspaces", id, "blobs", hash]) => {
            let device = device(service, &parts.headers).await?;
            let (id, hash) = ((*id).to_owned(), (*hash).to_owned());
            let engine = Arc::clone(service);
            let bytes = blocking(move || engine.engine.blob(&device, &id, &hash)).await?;
            let mut reply = Response::new(Full::new(Bytes::from(bytes)));
            headers(reply.headers_mut(), "application/octet-stream");
            Ok(reply)
        }
        (Method::GET, ["workspaces", id, "changes"]) => {
            let device = device(service, &parts.headers).await?;
            let (since, limit) = pull_query(parts.uri.query().unwrap_or(""))?;
            let id = (*id).to_owned();
            let engine = Arc::clone(service);
            json(
                StatusCode::OK,
                &blocking(move || engine.engine.pull(&device, &id, since, limit)).await?,
            )
        }
        _ => Err(not_found()),
    }
}

fn pull_query(query: &str) -> ServiceResult<(u64, Option<u32>)> {
    let mut since = 0;
    let mut limit = None;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = percent_encoding::percent_decode_str(value)
            .decode_utf8()
            .map_err(|_| invalid_query())?;
        match key {
            "since" => since = value.parse().map_err(|_| invalid_query())?,
            "limit" => limit = Some(value.parse().map_err(|_| invalid_query())?),
            _ => return Err(invalid_query()),
        }
    }
    Ok((since, limit))
}

fn invalid_query() -> ServiceError {
    ServiceError::new(
        400,
        "INVALID_REQUEST",
        "查询参数只能是 since 和 limit 的非负整数。",
    )
}

async fn blocking<T: Send + 'static>(
    task: impl FnOnce() -> ServiceResult<T> + Send + 'static,
) -> ServiceResult<T> {
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|_| ServiceError::new(500, "SERVICE_STATE", "同步服务处理请求时中断。"))?
}

fn bearer(headers: &HeaderMap) -> ServiceResult<&str> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next().ok_or_else(ServiceError::unauthorized)?;
    if values.next().is_some() {
        return Err(ServiceError::unauthorized());
    }
    value
        .to_str()
        .ok()
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
        .map(|(_, token)| token.trim())
        .filter(|token| !token.is_empty())
        .ok_or_else(ServiceError::unauthorized)
}

fn admin(service: &Service, headers: &HeaderMap) -> ServiceResult<()> {
    let candidate = digest(bearer(headers)?.as_bytes());
    // Both digests have the same length; compare without early exit.
    let difference = candidate
        .iter()
        .zip(&service.admin_digest)
        .fold(0_u8, |acc, (left, right)| acc | (left ^ right));
    if difference != 0 {
        return Err(ServiceError::new(401, "UNAUTHORIZED", "管理令牌无效。"));
    }
    Ok(())
}

async fn device(service: &Arc<Service>, headers: &HeaderMap) -> ServiceResult<Device> {
    let token = bearer(headers)?.to_owned();
    let engine = Arc::clone(service);
    blocking(move || engine.engine.authenticate(&token)).await
}

async fn read_json<T: DeserializeOwned>(headers: &HeaderMap, body: Incoming) -> ServiceResult<T> {
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
    if !json {
        return Err(ServiceError::new(
            415,
            "CONTENT_TYPE",
            "请求正文需要使用 application/json。",
        ));
    }
    let collected = timeout(
        REQUEST_TIMEOUT,
        Limited::new(body, MAX_REQUEST_BYTES).collect(),
    )
    .await
    .map_err(|_| ServiceError::new(408, "REQUEST_TIMEOUT", "读取请求正文超时。"))?
    .map_err(|error| {
        if error.is::<LengthLimitError>() {
            ServiceError::new(413, "REQUEST_TOO_LARGE", "请求正文最大为 32 MiB。")
        } else {
            ServiceError::new(400, "INVALID_REQUEST", "无法读取请求正文。")
        }
    })?;
    serde_json::from_slice(&collected.to_bytes()).map_err(|error| {
        ServiceError::new(400, "INVALID_REQUEST", format!("请求格式无效。{error}"))
    })
}

async fn read_bytes(headers: &HeaderMap, body: Incoming) -> ServiceResult<Bytes> {
    let binary = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .trim()
                .eq_ignore_ascii_case("application/octet-stream")
        });
    if !binary {
        return Err(ServiceError::new(
            415,
            "CONTENT_TYPE",
            "附件原件需要使用 application/octet-stream。",
        ));
    }
    let collected = timeout(
        REQUEST_TIMEOUT * 10,
        Limited::new(body, MAX_BLOB_BYTES as usize).collect(),
    )
    .await
    .map_err(|_| ServiceError::new(408, "REQUEST_TIMEOUT", "读取附件原件超时。"))?
    .map_err(|error| {
        if error.is::<LengthLimitError>() {
            ServiceError::new(413, "BLOB_TOO_LARGE", "附件原件最大为 100 MiB。")
        } else {
            ServiceError::new(400, "INVALID_REQUEST", "无法读取附件原件。")
        }
    })?;
    Ok(collected.to_bytes())
}

fn not_found() -> ServiceError {
    ServiceError::new(404, "NOT_FOUND", "没有这个同步接口。")
}

fn json(status: StatusCode, value: &impl Serialize) -> ServiceResult<Reply> {
    let body = serde_json::to_vec(value)?;
    let mut reply = Response::new(Full::new(Bytes::from(body)));
    *reply.status_mut() = status;
    headers(reply.headers_mut(), "application/json");
    Ok(reply)
}

fn empty(status: StatusCode) -> Reply {
    let mut reply = Response::new(Full::new(Bytes::new()));
    *reply.status_mut() = status;
    headers(reply.headers_mut(), "application/json");
    reply
}

fn headers(headers: &mut HeaderMap, content_type: &'static str) {
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
}

fn error_reply(error: &ServiceError) -> Reply {
    let status = StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = ErrorBody {
        code: error.code.into(),
        message: error.message.clone(),
    };
    let mut reply = json(status, &body).unwrap_or_else(|_| empty(status));
    if status == StatusCode::UNAUTHORIZED {
        reply
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    }
    reply
}

#[cfg(test)]
mod tests {
    use super::pull_query;

    #[test]
    fn parses_pull_query() {
        assert_eq!(pull_query("").unwrap(), (0, None));
        assert_eq!(pull_query("since=12&limit=3").unwrap(), (12, Some(3)));
        assert!(pull_query("since=-1").is_err());
        assert!(pull_query("cursor=1").is_err());
    }
}
