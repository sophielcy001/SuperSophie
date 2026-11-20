// SPDX-License-Identifier: AGPL-3.0-or-later

use futures_util::stream;
use http_body_util::{
    BodyExt, Full, LengthLimitError, Limited, StreamBody, combinators::UnsyncBoxBody,
};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Frame, Incoming},
    header::{self, HeaderMap, HeaderName, HeaderValue},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    convert::Infallible,
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncSeekExt, AsyncWriteExt},
    net::{TcpListener, TcpSocket},
    task::{JoinHandle, JoinSet},
    time::timeout,
};
use zhiye_core::{
    Error, IntegrationAssetDownload, IntegrationAssetUpload, IntegrationBlockOperation,
    IntegrationCollectionQuery, IntegrationMutation, IntegrationPage, IntegrationSearchHit,
    IntegrationSearchRequest, PageLocation, PagePatch, ProposalDraft, Result, Store,
    integration_markdown,
};

const MAX_BODY: usize = 1024 * 1024;
const MAX_CONNECTIONS: usize = 16;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(120);
const INTEGRATION_WINDOW_SECS: u64 = 10;
const INTEGRATION_REQUESTS: u32 = 60;
const PORT_BIND_ATTEMPTS: usize = 16;

// The Fetch standard blocks these ports even for loopback HTTP.
// https://fetch.spec.whatwg.org/#port-blocking
const FETCH_BLOCKED_PORTS: &[u16] = &[
    0, 1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95, 101,
    102, 103, 104, 109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179, 389, 427,
    465, 512, 513, 514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601, 636, 989, 990,
    993, 995, 1719, 1720, 1723, 2049, 3659, 4045, 4190, 5060, 5061, 6000, 6566, 6665, 6666, 6667,
    6668, 6669, 6679, 6697, 10080,
];

type Body = UnsyncBoxBody<Bytes, io::Error>;
type Reply = Response<Body>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalApiStatus {
    pub address: Option<String>,
}

pub struct LocalApi {
    address: SocketAddr,
    session: Arc<Session>,
    task: JoinHandle<()>,
}

struct Session {
    host: String,
    store: Arc<Store>,
    on_write: Box<dyn Fn(&str) + Send + Sync>,
    enabled: Mutex<bool>,
    rate: Mutex<HashMap<String, (Instant, u32)>>,
}

fn bind_local_listener() -> io::Result<TcpListener> {
    let mut rejected = Vec::new();
    for _ in 0..PORT_BIND_ATTEMPTS {
        let socket = TcpSocket::new_v4()?;
        socket.bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))?;
        if FETCH_BLOCKED_PORTS
            .binary_search(&socket.local_addr()?.port())
            .is_err()
        {
            return socket.listen(1024);
        }
        // Keep this port reserved until selection completes so it cannot be assigned again.
        rejected.push(socket);
    }
    Err(io::Error::new(
        io::ErrorKind::AddrNotAvailable,
        "no Fetch-compatible port available for the local API",
    ))
}

impl LocalApi {
    pub fn start(
        store: Arc<Store>,
        on_write: impl Fn(&str) + Send + Sync + 'static,
    ) -> Result<Self> {
        let listener = bind_local_listener()?;
        let address = listener.local_addr()?;
        let session = Arc::new(Session {
            host: address.to_string(),
            store,
            on_write: Box::new(on_write),
            enabled: Mutex::new(true),
            rate: Mutex::default(),
        });
        let task = tokio::spawn(serve(listener, Arc::clone(&session)));
        Ok(Self {
            address,
            session,
            task,
        })
    }

    pub fn status(instance: Option<&Self>) -> LocalApiStatus {
        LocalApiStatus {
            address: instance
                .filter(|api| !api.task.is_finished())
                .map(|api| format!("http://{}", api.address)),
        }
    }
}

impl Drop for LocalApi {
    fn drop(&mut self) {
        if let Ok(mut enabled) = self.session.enabled.lock() {
            *enabled = false;
        }
        self.task.abort();
    }
}

impl Session {
    async fn run<T: Send + 'static>(
        self: &Arc<Self>,
        job: impl FnOnce(&Store) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let session = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let enabled = session.enabled.lock().map_err(|_| service_stopped())?;
            if !*enabled {
                return Err(service_stopped());
            }
            job(&session.store)
        })
        .await
        .map_err(|_| Error::new("API_TASK", "本机 API 操作失败。"))?
    }

    async fn write<T: Send + 'static>(
        self: &Arc<Self>,
        credentials: Arc<Credentials>,
        job: impl FnOnce(&Store, &Credentials) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let session = Arc::clone(self);
        self.run(move |store| {
            let result = job(store, &credentials)?;
            // A disconnected client can drop the response future after the commit.
            (session.on_write)(&credentials.workspace);
            Ok(result)
        })
        .await
    }

    async fn admit(self: &Arc<Self>, credentials: Arc<Credentials>) -> Result<()> {
        let identity = self
            .run(move |store| {
                store.integration_identity(&credentials.workspace, &credentials.token)
            })
            .await?;
        let mut map = self.rate.lock().map_err(|_| service_stopped())?;
        let now = Instant::now();
        let interval = Duration::from_secs(INTEGRATION_WINDOW_SECS);
        map.retain(|_, (started, _)| now.duration_since(*started) < interval);
        let (_, count) = map.entry(identity).or_insert((now, 0));
        if *count >= INTEGRATION_REQUESTS {
            return Err(Error::new("RATE_LIMIT", "请求过于频繁，请稍后重试。"));
        }
        *count += 1;
        Ok(())
    }
}

async fn serve(listener: TcpListener, session: Arc<Session>) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            _ = connections.join_next(), if !connections.is_empty() => {}
            accepted = listener.accept(), if connections.len() < MAX_CONNECTIONS => {
                let Ok((socket, peer)) = accepted else { return };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let session = Arc::clone(&session);
                connections.spawn(async move {
                    let service = service_fn(move |request| {
                        let session = Arc::clone(&session);
                        async move {
                            let reply = match dispatch(&session, request).await {
                                Ok(reply) => reply,
                                Err(error) => error_reply(error),
                            };
                            Ok::<_, Infallible>(reply)
                        }
                    });
                    let mut http = http1::Builder::new();
                    http.timer(TokioTimer::new())
                        .header_read_timeout(REQUEST_TIMEOUT)
                        .max_buf_size(16 * 1024)
                        .max_headers(32)
                        .keep_alive(false);
                    let _ = timeout(CONNECTION_TIMEOUT, http.serve_connection(TokioIo::new(socket), service)).await;
                });
            }
        }
    }
}

struct Credentials {
    workspace: String,
    token: String,
}

async fn dispatch(session: &Arc<Session>, request: Request<Incoming>) -> Result<Reply> {
    let (parts, body) = request.into_parts();
    let headers = &parts.headers;
    if one_header(headers, header::HOST)? != Some(session.host.as_str())
        || headers.contains_key(header::ORIGIN)
        || parts.uri.authority().is_some()
        || parts.uri.scheme().is_some()
    {
        return Err(Error::new(
            "REQUEST_ORIGIN",
            "本机 API 拒绝该 Host 或 Origin。",
        ));
    }
    let token = bearer(headers)?;
    let path: Vec<_> = parts.uri.path().split('/').collect();
    let ["", "api", "v1", "workspaces", workspace, endpoint @ ..] = path.as_slice() else {
        return Err(not_found());
    };
    let credentials = Arc::new(Credentials {
        workspace: (*workspace).into(),
        token: token.into(),
    });
    let query = parts.uri.query();
    if query.is_some()
        && !matches!(
            (parts.method.as_str(), endpoint),
            (
                "GET",
                ["pages", _, "content" | "backlinks"] | ["catalog"] | ["changes"]
            )
        )
    {
        return Err(invalid_request());
    }
    session.admit(Arc::clone(&credentials)).await?;
    match (parts.method, endpoint) {
        (Method::GET, ["integration"]) => {
            let scopes = session
                .run(move |store| {
                    store.integration_scopes(&credentials.workspace, &credentials.token)
                })
                .await?;
            json_reply(StatusCode::OK, &json!({ "scopes": scopes }))
        }
        (Method::GET, ["catalog"]) => {
            let ReadQuery { limit, cursor, .. } = read_query(query, false)?;
            let value = session
                .run(move |store| {
                    store.integration_catalog(
                        &credentials.workspace,
                        &credentials.token,
                        limit,
                        cursor.as_deref(),
                    )
                })
                .await?;
            json_reply(StatusCode::OK, &value)
        }
        (Method::GET, ["changes"]) => {
            let ReadQuery { limit, cursor, .. } = read_query(query, false)?;
            let cursor = cursor.ok_or_else(invalid_request)?;
            let value = session
                .run(move |store| {
                    store.integration_changes(
                        &credentials.workspace,
                        &credentials.token,
                        limit,
                        &cursor,
                    )
                })
                .await?;
            json_reply(StatusCode::OK, &value)
        }
        (Method::POST, ["proposals"]) => {
            let draft: ProposalDraft = read_json(headers, body).await?;
            let key = one_header(headers, HeaderName::from_static("idempotency-key"))?
                .unwrap_or_default()
                .to_owned();
            let receipt = session
                .run(move |store| {
                    store.submit_integration_proposal(
                        &credentials.workspace,
                        &credentials.token,
                        &key,
                        draft,
                    )
                })
                .await?;
            json_reply(StatusCode::CREATED, &receipt)
        }
        (Method::GET, ["proposals", id]) => {
            let id = (*id).to_owned();
            let receipt = session
                .run(move |store| {
                    store.integration_proposal(&credentials.workspace, &credentials.token, &id)
                })
                .await?;
            json_reply(StatusCode::OK, &receipt)
        }
        (Method::POST, ["search"]) => {
            let request: IntegrationSearchRequest = read_json(headers, body).await?;
            let workspace = credentials.workspace.clone();
            let value = session
                .run(move |store| {
                    store.integration_search(&credentials.workspace, &credentials.token, &request)
                })
                .await?;
            let items: Vec<_> = value
                .items
                .into_iter()
                .map(|hit| {
                    let location = PageLocation::page(&workspace, &hit.page_id);
                    let page_link = location.to_string();
                    let block_link = hit
                        .block_id
                        .as_deref()
                        .map(|id| location.block(id).to_string());
                    let page_path = resource_path(&workspace, "pages", &hit.page_id);
                    let block_path = hit
                        .block_id
                        .as_ref()
                        .map(|id| resource_path(&workspace, "blocks", id));
                    let pdf = hit.pdf.as_ref().map(|source| {
                        let target = &source.target;
                        PdfSearchLocation {
                            attachment_id: target.attachment_id.clone(),
                            sha256: target.sha256.clone(),
                            page_number: target.page_number,
                            name: source.name.clone(),
                            link: location
                                .pdf(&target.attachment_id, &target.sha256, target.page_number)
                                .to_string(),
                            content_path: format!(
                                "{}/content",
                                resource_path(&workspace, "assets", &target.attachment_id)
                            ),
                        }
                    });
                    SearchLocation {
                        hit,
                        page_path,
                        block_path,
                        page_link,
                        block_link,
                        pdf,
                    }
                })
                .collect();
            let mut payload = json!({
                "items": items,
                "snapshot_revision": value.snapshot_revision,
                "next_cursor": value.next_cursor,
            });
            if let Some(index) = value.pdf_index {
                payload["pdf_index"] = json!({
                    "pending": index.pending, "indexed": index.indexed, "partial": index.partial,
                    "no_text": index.no_text, "failed": index.failed,
                });
            }
            json_reply(StatusCode::OK, &payload)
        }
        (Method::GET, ["collections", id]) => {
            let id = (*id).to_owned();
            let value = session
                .run(move |store| {
                    store.integration_collection(&credentials.workspace, &credentials.token, &id)
                })
                .await?;
            json_reply(StatusCode::OK, &value)
        }
        (Method::POST, ["collections", id, "query"]) => {
            let id = (*id).to_owned();
            let request: IntegrationCollectionQuery = read_json(headers, body).await?;
            let value = session
                .run(move |store| {
                    store.integration_collection_query(
                        &credentials.workspace,
                        &credentials.token,
                        &id,
                        &request,
                    )
                })
                .await?;
            json_reply(StatusCode::OK, &value)
        }
        (Method::GET, ["pages", id]) => {
            let page_path = resource_path(&credentials.workspace, "pages", id);
            let page_link = PageLocation::page(&credentials.workspace, id).to_string();
            let page = read_page(session, credentials, (*id).into()).await?;
            let mut reply = json_reply(
                StatusCode::OK,
                &json!({
                    "id": page.id,
                    "page_path": page_path,
                    "page_link": page_link,
                    "parent_id": page.parent_id,
                    "title": page.title,
                    "icon": page.icon,
                    "page_revision": page.page_revision,
                    "updated_at": page.updated_at,
                    "schema_revision": page.schema_revision,
                    "properties": page.properties,
                    "properties_restricted": page.properties_restricted,
                    "restricted_property_ids": page.restricted_property_ids,
                }),
            )?;
            set_etag(&mut reply, &page.etag)?;
            Ok(reply)
        }
        (Method::GET, ["pages", id, "content"]) => {
            let page = read_page(session, Arc::clone(&credentials), (*id).into()).await?;
            content_reply(page, &credentials.workspace, query)
        }
        (Method::GET, ["pages", id, "computed"]) => {
            let id = (*id).to_owned();
            let value = session
                .run(move |store| {
                    store.integration_computed(&credentials.workspace, &credentials.token, &id)
                })
                .await?;
            json_reply(StatusCode::OK, &value)
        }
        (Method::GET, ["pages", id, "backlinks"]) => {
            let id = (*id).to_owned();
            let ReadQuery { limit, cursor, .. } = read_query(query, false)?;
            let value = session
                .run(move |store| {
                    store.integration_backlinks(
                        &credentials.workspace,
                        &credentials.token,
                        &id,
                        cursor.as_deref(),
                        limit,
                    )
                })
                .await?;
            json_reply(StatusCode::OK, &value)
        }
        (Method::GET, ["pages", id, "references", block]) => {
            let id = (*id).to_owned();
            let block = (*block).to_owned();
            let value = session
                .run(move |store| {
                    store.integration_reference(
                        &credentials.workspace,
                        &credentials.token,
                        &id,
                        &block,
                    )
                })
                .await?;
            json_reply(StatusCode::OK, &value)
        }
        (Method::GET, ["blocks", id]) => {
            let id = (*id).to_owned();
            let workspace = credentials.workspace.clone();
            let block_path = resource_path(&workspace, "blocks", &id);
            let value = session
                .run(move |store| {
                    store.integration_block(&credentials.workspace, &credentials.token, &id)
                })
                .await?;
            let page_path = resource_path(&workspace, "pages", &value.page_id);
            let location = PageLocation::page(&workspace, &value.page_id);
            json_reply(
                StatusCode::OK,
                &json!({
                    "page_id": value.page_id,
                    "page_revision": value.page_revision,
                    "page_path": page_path,
                    "block_path": block_path,
                    "page_link": location.to_string(),
                    "block_link": location.block(&value.block.id).to_string(),
                    "block": value.block,
                }),
            )
        }
        (Method::GET, ["assets", id, "content"]) => {
            download(session, credentials, (*id).into()).await
        }
        (Method::POST, ["assets"]) => upload(session, credentials, headers, body).await,
        (Method::POST, ["pages"]) => {
            let request: CreatePage = read_json(headers, body).await?;
            let icon = request.icon.unwrap_or_else(|| "file".into());
            let mutation = match (request.parent_id, request.collection_id, request.properties) {
                (Some(parent_id), None, None) => IntegrationMutation::CreatePage {
                    parent_id,
                    title: request.title,
                    icon,
                },
                (None, Some(collection_id), properties) => IntegrationMutation::CreateRecord {
                    collection_id,
                    title: request.title,
                    icon,
                    properties: properties.unwrap_or_default(),
                },
                _ => return Err(invalid_request()),
            };
            mutate(session, credentials, headers, mutation, StatusCode::CREATED).await
        }
        (Method::PATCH, ["pages", id]) => {
            let request: UpdatePage = read_json(headers, body).await?;
            let patch = PagePatch {
                title: request.title,
                icon: request.icon,
            };
            let mutation = match request.properties {
                Some(properties) => IntegrationMutation::PatchProperties {
                    page_id: (*id).into(),
                    patch,
                    properties,
                },
                None => IntegrationMutation::PatchPage {
                    page_id: (*id).into(),
                    patch,
                },
            };
            mutate(session, credentials, headers, mutation, StatusCode::OK).await
        }
        (Method::POST, ["pages", id, "archive"]) => {
            let mutation = IntegrationMutation::ArchivePage {
                page_id: (*id).into(),
            };
            mutate(session, credentials, headers, mutation, StatusCode::OK).await
        }
        (Method::POST, ["pages", id, "block-operations"]) => {
            let request: BlockOperations = read_json(headers, body).await?;
            // Keep the original operation digest for clients retrying a
            // plain-text append accepted by the 0.10 preview.
            let legacy = request
                .operations
                .iter()
                .map(|operation| operation.append_text().map(str::to_owned))
                .collect::<Option<Vec<_>>>();
            let mutation = if let Some(paragraphs) = legacy {
                IntegrationMutation::AppendParagraphs {
                    page_id: (*id).into(),
                    paragraphs,
                }
            } else {
                IntegrationMutation::EditBlocks {
                    page_id: (*id).into(),
                    operations: request.operations,
                }
            };
            mutate(session, credentials, headers, mutation, StatusCode::OK).await
        }
        _ => Err(not_found()),
    }
}

#[derive(Serialize)]
struct SearchLocation {
    #[serde(flatten)]
    hit: IntegrationSearchHit,
    page_path: String,
    block_path: Option<String>,
    page_link: String,
    block_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pdf: Option<PdfSearchLocation>,
}

#[derive(Serialize)]
struct PdfSearchLocation {
    attachment_id: String,
    sha256: String,
    page_number: u32,
    name: String,
    link: String,
    content_path: String,
}

fn resource_path(workspace: &str, kind: &str, id: &str) -> String {
    format!("/api/v1/workspaces/{workspace}/{kind}/{id}")
}

async fn read_page(
    session: &Arc<Session>,
    credentials: Arc<Credentials>,
    id: String,
) -> Result<IntegrationPage> {
    session
        .run(move |store| store.integration_page(&credentials.workspace, &credentials.token, &id))
        .await
}

struct ReadQuery {
    limit: Option<usize>,
    cursor: Option<String>,
    markdown: bool,
}

fn read_query(query: Option<&str>, has_format: bool) -> Result<ReadQuery> {
    let mut limit = None;
    let mut cursor = None;
    let mut format = None;
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        match key.as_ref() {
            "limit" if limit.is_none() => {
                limit = Some(
                    value
                        .parse::<usize>()
                        .ok()
                        .filter(|n| (1..=100).contains(n))
                        .ok_or_else(invalid_request)?,
                );
            }
            "cursor" if cursor.is_none() => cursor = Some(value.into_owned()),
            "format"
                if has_format
                    && format.is_none()
                    && matches!(value.as_ref(), "blocks" | "markdown") =>
            {
                format = Some(value)
            }
            _ => return Err(invalid_request()),
        }
    }
    Ok(ReadQuery {
        limit,
        cursor,
        markdown: format.as_deref() == Some("markdown"),
    })
}

fn content_reply(page: IntegrationPage, workspace: &str, query: Option<&str>) -> Result<Reply> {
    let ReadQuery {
        limit,
        cursor,
        markdown,
    } = read_query(query, true)?;
    let tag = page.etag.trim_matches('"');
    let cursor_tag = if markdown {
        format!("{tag}.markdown")
    } else {
        tag.to_owned()
    };
    let offset = cursor
        .as_deref()
        .map(|cursor| {
            let (index, provided_tag) = cursor.split_once('.').ok_or_else(invalid_cursor)?;
            if provided_tag != cursor_tag {
                return Err(invalid_cursor());
            }
            index
                .parse::<usize>()
                .ok()
                .filter(|index| *index < page.blocks.len())
                .ok_or_else(invalid_cursor)
        })
        .transpose()?
        .unwrap_or(0);
    let end = (offset + limit.unwrap_or(50)).min(page.blocks.len());
    let next_cursor = (end < page.blocks.len()).then(|| format!("{end}.{cursor_tag}"));
    let blocks = &page.blocks[offset..end];
    let representation = if markdown {
        let text = integration_markdown(
            workspace,
            (offset == 0).then_some(page.title.as_str()),
            blocks,
        );
        json!({
            "format": "markdown",
            "page_id": page.id,
            "page_revision": page.page_revision,
            "markdown": text.markdown,
            "losses": text.losses,
            "next_cursor": next_cursor,
        })
    } else {
        json!({
            "format": "blocks",
            "page_id": page.id,
            "page_revision": page.page_revision,
            "blocks": blocks,
            "next_cursor": next_cursor,
        })
    };
    let mut reply = json_reply(StatusCode::OK, &representation)?;
    set_etag(&mut reply, &page.etag)?;
    Ok(reply)
}

async fn mutate(
    session: &Arc<Session>,
    credentials: Arc<Credentials>,
    headers: &HeaderMap,
    mutation: IntegrationMutation,
    status: StatusCode,
) -> Result<Reply> {
    let key = one_header(headers, HeaderName::from_static("idempotency-key"))?
        .unwrap_or_default()
        .to_owned();
    let etag = one_header(headers, header::IF_MATCH)?.map(str::to_owned);
    let expected_revision = one_header(headers, HeaderName::from_static("x-zhiye-base-revision"))?
        .map(|value| {
            value
                .parse::<u32>()
                .ok()
                .filter(|&revision| revision > 0)
                .ok_or_else(invalid_request)
        })
        .transpose()?;
    let result = session
        .write(credentials, move |store, credentials| {
            store.mutate_as_integration_at(
                &credentials.workspace,
                &credentials.token,
                &key,
                etag.as_deref(),
                expected_revision,
                mutation,
            )
        })
        .await?;
    json_reply(status, &result)
}

async fn upload(
    session: &Arc<Session>,
    credentials: Arc<Credentials>,
    headers: &HeaderMap,
    body: Incoming,
) -> Result<Reply> {
    if !media_type(headers)?
        .is_some_and(|kind| kind.eq_ignore_ascii_case("application/octet-stream"))
    {
        return Err(Error::new(
            "CONTENT_TYPE",
            "附件正文需要使用 application/octet-stream。",
        ));
    }
    let page_id = required_header(headers, "x-zhiye-page-id")?.to_owned();
    let name = percent_encoding::percent_decode_str(required_header(headers, "x-zhiye-file-name")?)
        .decode_utf8()
        .map_err(|_| invalid_request())?
        .into_owned();
    let key = one_header(headers, HeaderName::from_static("idempotency-key"))?
        .unwrap_or_default()
        .to_owned();
    let etag = one_header(headers, header::IF_MATCH)?.map(str::to_owned);

    let fields = Arc::new((page_id, name, key, etag));
    let preliminary = Arc::clone(&fields);
    let authorization = Arc::clone(&credentials);
    let limit = session
        .run(move |store| {
            let (page_id, name, request_key, etag) = preliminary.as_ref();
            store.integration_upload_limit(
                &authorization.workspace,
                &authorization.token,
                &IntegrationAssetUpload {
                    page_id,
                    name,
                    request_key,
                    if_match: etag.as_deref(),
                },
            )
        })
        .await?;
    let file = buffer_upload(body, limit as usize).await?;
    let saved = session
        .write(credentials, move |store, credentials| {
            let (page_id, name, request_key, etag) = fields.as_ref();
            store.upload_as_integration(
                &credentials.workspace,
                &credentials.token,
                IntegrationAssetUpload {
                    page_id,
                    name,
                    request_key,
                    if_match: etag.as_deref(),
                },
                file,
            )
        })
        .await?;
    json_reply(StatusCode::CREATED, &saved)
}

async fn buffer_upload(body: Incoming, limit: usize) -> Result<std::fs::File> {
    timeout(Duration::from_secs(60), async move {
        let mut file = tokio::fs::File::from_std(tempfile::tempfile()?);
        let mut stream = Limited::new(body, limit);
        while let Some(frame) = stream.frame().await {
            let frame = frame.map_err(|error| {
                if error.is::<LengthLimitError>() {
                    Error::new("FILE_TOO_LARGE", "附件超过允许的文件大小。")
                } else {
                    invalid_request()
                }
            })?;
            if let Ok(bytes) = frame.into_data() {
                file.write_all(&bytes).await?;
            }
        }
        file.flush().await?;
        file.seek(io::SeekFrom::Start(0)).await?;
        Ok(file.into_std().await)
    })
    .await
    .map_err(|_| Error::new("REQUEST_TIMEOUT", "读取附件正文超时。"))?
}

struct Download {
    session: Arc<Session>,
    credentials: Arc<Credentials>,
    asset: IntegrationAssetDownload,
}

async fn download(
    session: &Arc<Session>,
    credentials: Arc<Credentials>,
    id: String,
) -> Result<Reply> {
    let authorization = Arc::clone(&credentials);
    let asset = session
        .run(move |store| {
            store.open_integration_asset(&authorization.workspace, &authorization.token, &id)
        })
        .await?;
    let size = asset.attachment().bytes;
    let download = Download {
        session: Arc::clone(session),
        credentials,
        asset,
    };
    let chunks = stream::try_unfold(download, |mut download| async {
        let session = Arc::clone(&download.session);
        let (bytes, download) = session
            .run(move |store| {
                let bytes = store.read_integration_asset_chunk(
                    &download.credentials.workspace,
                    &download.credentials.token,
                    &mut download.asset,
                )?;
                Ok((bytes, download))
            })
            .await
            .map_err(|_| io::Error::other("Attachment access ended"))?;
        Ok(bytes.map(|bytes| (Frame::data(Bytes::from(bytes)), download)))
    });
    let mut reply = response(
        StatusCode::OK,
        "application/octet-stream",
        StreamBody::new(chunks).boxed_unsync(),
    );
    reply
        .headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    reply.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment"),
    );
    Ok(reply)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePage {
    parent_id: Option<String>,
    collection_id: Option<String>,
    title: String,
    icon: Option<String>,
    properties: Option<BTreeMap<String, Value>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdatePage {
    title: Option<String>,
    icon: Option<String>,
    properties: Option<BTreeMap<String, Value>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockOperations {
    operations: Vec<IntegrationBlockOperation>,
}

fn one_header(headers: &HeaderMap, name: HeaderName) -> Result<Option<&str>> {
    let values = headers.get_all(name);
    let mut values = values.iter();
    let first = values.next();
    if values.next().is_some() {
        return Err(invalid_request());
    }
    first
        .map(|value| value.to_str().map_err(|_| invalid_request()))
        .transpose()
}

fn required_header<'a>(headers: &'a HeaderMap, name: &'static str) -> Result<&'a str> {
    one_header(headers, HeaderName::from_static(name))?.ok_or_else(invalid_request)
}

fn bearer(headers: &HeaderMap) -> Result<&str> {
    one_header(headers, header::AUTHORIZATION)?
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
        .map(|(_, token)| token.trim_start_matches(' '))
        .filter(|token| !token.is_empty())
        .ok_or_else(|| Error::new("UNAUTHORIZED", "请在 Authorization 中提供集成令牌。"))
}

async fn read_json<T: DeserializeOwned>(headers: &HeaderMap, body: Incoming) -> Result<T> {
    if !media_type(headers)?
        .is_some_and(|media_type| media_type.eq_ignore_ascii_case("application/json"))
    {
        return Err(Error::new(
            "CONTENT_TYPE",
            "请求正文需要使用 application/json。",
        ));
    }
    let collected = timeout(REQUEST_TIMEOUT, Limited::new(body, MAX_BODY).collect())
        .await
        .map_err(|_| Error::new("REQUEST_TIMEOUT", "读取请求正文超时。"))?
        .map_err(|error| {
            if error.is::<LengthLimitError>() {
                Error::new("REQUEST_TOO_LARGE", "JSON 请求最大为 1 MiB。")
            } else {
                invalid_request()
            }
        })?;
    serde_json::from_slice(&collected.to_bytes()).map_err(|_| invalid_request())
}

fn media_type(headers: &HeaderMap) -> Result<Option<&str>> {
    Ok(one_header(headers, header::CONTENT_TYPE)?
        .and_then(|content| content.split(';').next())
        .map(str::trim))
}

fn set_etag(reply: &mut Reply, tag: &str) -> Result<()> {
    let value = HeaderValue::from_str(tag)
        .map_err(|_| Error::new("API_RESPONSE", "无法生成页面修订标识。"))?;
    reply.headers_mut().insert(header::ETAG, value);
    Ok(())
}

fn response(status: StatusCode, content_type: &'static str, body: Body) -> Reply {
    let mut reply = Response::new(body);
    *reply.status_mut() = status;
    let headers = reply.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    reply
}

fn json_reply(status: StatusCode, data: &impl Serialize) -> Result<Reply> {
    let bytes = Bytes::from(serde_json::to_vec(data)?);
    let body = Full::new(bytes)
        .map_err(|never| match never {})
        .boxed_unsync();
    Ok(response(status, "application/json; charset=utf-8", body))
}

fn error_reply(error: Error) -> Reply {
    let status = match error.code {
        "UNAUTHORIZED" => StatusCode::UNAUTHORIZED,
        "FORBIDDEN" | "PARTIAL_CONTENT" | "REQUEST_ORIGIN" => StatusCode::FORBIDDEN,
        "RESOURCE_NOT_FOUND" => StatusCode::NOT_FOUND,
        "PRECONDITION_REQUIRED" => StatusCode::PRECONDITION_REQUIRED,
        "REVISION_CONFLICT" => StatusCode::PRECONDITION_FAILED,
        "IDEMPOTENCY_CONFLICT" | "CURSOR_INVALID" => StatusCode::CONFLICT,
        "CURSOR_EXPIRED" => StatusCode::GONE,
        "REQUEST_TOO_LARGE" | "PAGE_TOO_LARGE" | "FILE_TOO_LARGE" | "TOO_MANY_OPERATIONS" => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        "CONTENT_TYPE" => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "REQUEST_TIMEOUT" => StatusCode::REQUEST_TIMEOUT,
        "API_STOPPED" => StatusCode::SERVICE_UNAVAILABLE,
        "RATE_LIMIT" => StatusCode::TOO_MANY_REQUESTS,
        "IDEMPOTENCY_KEY"
        | "INVALID_REQUEST"
        | "INVALID_PATCH"
        | "INVALID_NAME"
        | "INVALID_ICON"
        | "INVALID_ATTACHMENT"
        | "INVALID_BLOCK"
        | "INVALID_BLOCK_OPERATION"
        | "QUERY_LIMIT"
        | "QUERY_FILTER_LIMIT"
        | "SEARCH_SCOPE"
        | "SEARCH_QUERY"
        | "VIEW_FILTER"
        | "VIEW_SORT"
        | "PROPERTY_VALUE"
        | "BLOCK_ATTACHMENT"
        | "RELATION_DELTA"
        | "RECORD_NOT_FOUND"
        | "PROPERTY_NOT_FOUND" => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let message = if status == StatusCode::INTERNAL_SERVER_ERROR {
        "无法完成本机 API 请求。"
    } else {
        error.message.as_str()
    };
    let output = json!({
        "code": error.code,
        "message": message,
        "request_id": uuid::Uuid::new_v4().to_string(),
        "retryable": matches!(status, StatusCode::REQUEST_TIMEOUT | StatusCode::SERVICE_UNAVAILABLE | StatusCode::TOO_MANY_REQUESTS),
        "details": null,
    });
    let mut reply = json_reply(status, &output).unwrap_or_else(|_| {
        response(
            status,
            "application/json; charset=utf-8",
            Full::new(Bytes::new())
                .map_err(|never| match never {})
                .boxed_unsync(),
        )
    });
    if status == StatusCode::UNAUTHORIZED {
        reply
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        reply.headers_mut().insert(
            header::RETRY_AFTER,
            HeaderValue::from(INTEGRATION_WINDOW_SECS),
        );
    }
    reply
}

fn not_found() -> Error {
    Error::new("RESOURCE_NOT_FOUND", "找不到该资源。")
}

fn invalid_request() -> Error {
    Error::new("INVALID_REQUEST", "请求格式无效。")
}

fn invalid_cursor() -> Error {
    Error::new(
        "CURSOR_INVALID",
        "页面内容或授权已更新，请从第一页重新读取。",
    )
}

fn service_stopped() -> Error {
    Error::new("API_STOPPED", "本机 API 已关闭。")
}
