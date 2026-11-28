// SPDX-License-Identifier: AGPL-3.0-or-later

//! The service over real loopback HTTP, driven by two local stores.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use zhiye_core::Store;
use zhiye_core::sync_protocol::{IssuedDevice, PullResponse, PushResponse, WorkspaceInfo};
use zhiye_sync_server::{Engine, Service, serve};

const ADMIN: &str = "test-admin-token-0123456789abcdef0123";

fn start() -> (SocketAddr, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let service = Arc::new(Service::new(Engine::open_for_tests().unwrap(), ADMIN).unwrap());
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            sender.send(listener.local_addr().unwrap()).unwrap();
            serve(listener, service).await;
        });
    });
    (receiver.recv().unwrap(), directory)
}

/// Minimal HTTP/1.1 client: one request per connection.
fn request(
    address: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&impl Serialize>,
) -> (u16, Value) {
    let body = body.map(|value| serde_json::to_vec(value).unwrap());
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n");
    if let Some(token) = token {
        head.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if let Some(body) = &body {
        head.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    head.push_str("\r\n");
    let mut stream = TcpStream::connect(address).unwrap();
    stream.write_all(head.as_bytes()).unwrap();
    if let Some(body) = &body {
        stream.write_all(body).unwrap();
    }
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let text = String::from_utf8(response).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(body).unwrap()
    };
    (status, value)
}

fn ok<T: DeserializeOwned>((status, value): (u16, Value)) -> T {
    assert!((200..300).contains(&status), "{status}: {value}");
    serde_json::from_value(value).unwrap()
}

fn sync(address: SocketAddr, token: &str, store: &Store, workspace: &str) {
    let pull = || loop {
        let cursor = store.sync_status(workspace).unwrap().cursor;
        let page: PullResponse = ok(request(
            address,
            "GET",
            &format!("/v1/workspaces/{workspace}/changes?since={cursor}&limit=1"),
            Some(token),
            None::<&()>,
        ));
        store.sync_apply_pull(workspace, &page).unwrap();
        if !page.has_more {
            break;
        }
    };
    pull();
    while let Some(push) = store.sync_prepare_push(workspace).unwrap().request {
        let response: PushResponse = ok(request(
            address,
            "POST",
            &format!("/v1/workspaces/{workspace}/push"),
            Some(token),
            Some(&push),
        ));
        store.sync_apply_push(workspace, &response).unwrap();
    }
    pull();
}

#[test]
fn devices_sync_over_http_and_revocation_blocks_access() {
    let (address, _data) = start();
    let health = request(address, "GET", "/v1/health", None, None::<&()>);
    assert_eq!(health.0, 200);
    assert_eq!(health.1["protocol"], 1);

    let denied = request(
        address,
        "POST",
        "/v1/devices",
        Some("wrong"),
        Some(&json!({"name": "x"})),
    );
    assert_eq!(denied.0, 401);
    let first: IssuedDevice = ok(request(
        address,
        "POST",
        "/v1/devices",
        Some(ADMIN),
        Some(&json!({"name": "书房电脑"})),
    ));
    let second: IssuedDevice = ok(request(
        address,
        "POST",
        "/v1/devices",
        Some(ADMIN),
        Some(&json!({"name": "笔记本"})),
    ));
    // A device token cannot manage devices.
    let escalation = request(
        address,
        "GET",
        "/v1/devices",
        Some(&first.token),
        None::<&()>,
    );
    assert_eq!(escalation.0, 401);

    let one = tempfile::tempdir().unwrap();
    let two = tempfile::tempdir().unwrap();
    let first_store = Store::open(one.path()).unwrap();
    let second_store = Store::open(two.path()).unwrap();
    let workspace = first_store.create_workspace("个人资料").unwrap();
    let _: WorkspaceInfo = ok(request(
        address,
        "PUT",
        &format!("/v1/workspaces/{}", workspace.id),
        Some(&first.token),
        Some(&json!({"name": workspace.name})),
    ));
    let parent = first_store
        .create_page(&workspace.id, None, "项目", "📁")
        .unwrap();
    for index in 0..3 {
        first_store
            .create_page(
                &workspace.id,
                Some(&parent.info.id),
                &format!("第 {index} 页"),
                "📄",
            )
            .unwrap();
    }
    sync(address, &first.token, &first_store, &workspace.id);

    let listed: Vec<WorkspaceInfo> = ok(request(
        address,
        "GET",
        "/v1/workspaces",
        Some(&second.token),
        None::<&()>,
    ));
    assert_eq!(listed.len(), 1);
    second_store
        .create_synced_workspace(&listed[0].id, &listed[0].name)
        .unwrap();
    sync(address, &second.token, &second_store, &workspace.id);
    let titles = |store: &Store| {
        let mut titles: Vec<_> = store
            .pages(&workspace.id)
            .unwrap()
            .into_iter()
            .map(|page| (page.id, page.parent_id, page.title))
            .collect();
        titles.sort();
        titles
    };
    assert_eq!(titles(&first_store), titles(&second_store));
    assert_eq!(second_store.pages(&workspace.id).unwrap().len(), 4);

    let revoked = request(
        address,
        "DELETE",
        &format!("/v1/devices/{}", second.device.id),
        Some(ADMIN),
        None::<&()>,
    );
    assert_eq!(revoked.0, 204);
    let blocked = request(
        address,
        "GET",
        &format!("/v1/workspaces/{}/changes", workspace.id),
        Some(&second.token),
        None::<&()>,
    );
    assert_eq!(blocked.0, 401);
    assert_eq!(blocked.1["code"], "UNAUTHORIZED");
    let devices: Vec<Value> = ok(request(
        address,
        "GET",
        "/v1/devices",
        Some(ADMIN),
        None::<&()>,
    ));
    assert!(devices.iter().any(|device| !device["revokedAt"].is_null()));

    let malformed = request(
        address,
        "POST",
        &format!("/v1/workspaces/{}/push", workspace.id),
        Some(&first.token),
        Some(&json!({"protocol": 1, "operations": []})),
    );
    assert_eq!(malformed.0, 400);
    let unknown = request(address, "GET", "/v2/health", None, None::<&()>);
    assert_eq!(unknown.0, 404);
}

/// Raw HTTP/1.1 exchange for binary bodies.
fn raw(
    address: SocketAddr,
    method: &str,
    path: &str,
    token: &str,
    body: Option<(&str, &[u8])>,
) -> (u16, Vec<u8>) {
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nAuthorization: Bearer {token}\r\n"
    );
    if let Some((content_type, bytes)) = body {
        head.push_str(&format!(
            "Content-Type: {content_type}\r\nContent-Length: {}\r\n",
            bytes.len()
        ));
    }
    head.push_str("\r\n");
    let mut stream = TcpStream::connect(address).unwrap();
    stream.write_all(head.as_bytes()).unwrap();
    if let Some((_, bytes)) = body {
        stream.write_all(bytes).unwrap();
    }
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let head = String::from_utf8_lossy(&response[..split]).to_string();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, response[split + 4..].to_vec())
}

#[test]
fn originals_travel_over_http() {
    use sha2::{Digest, Sha256};
    let (address, _data) = start();
    let device: IssuedDevice = ok(request(
        address,
        "POST",
        "/v1/devices",
        Some(ADMIN),
        Some(&json!({"name": "书房电脑"})),
    ));
    let workspace = uuid::Uuid::new_v4().to_string();
    let _: WorkspaceInfo = ok(request(
        address,
        "PUT",
        &format!("/v1/workspaces/{workspace}"),
        Some(&device.token),
        Some(&json!({"name": "原件"})),
    ));
    let bytes = (0..=255_u8).cycle().take(300_000).collect::<Vec<_>>();
    let hash: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let missing: Value = ok(request(
        address,
        "POST",
        &format!("/v1/workspaces/{workspace}/blobs/missing"),
        Some(&device.token),
        Some(&json!({"hashes": [hash]})),
    ));
    assert_eq!(missing["missing"], json!([hash]));

    let path = format!("/v1/workspaces/{workspace}/blobs/{hash}");
    let wrong_type = raw(
        address,
        "PUT",
        &path,
        &device.token,
        Some(("text/plain", b"x")),
    );
    assert_eq!(wrong_type.0, 415);
    let mismatch = raw(
        address,
        "PUT",
        &path,
        &device.token,
        Some(("application/octet-stream", b"x")),
    );
    assert_eq!(mismatch.0, 400);
    let stored = raw(
        address,
        "PUT",
        &path,
        &device.token,
        Some(("application/octet-stream", &bytes)),
    );
    assert_eq!(stored.0, 204);
    let fetched = raw(address, "GET", &path, &device.token, None);
    assert_eq!((fetched.0, fetched.1.len()), (200, bytes.len()));
    assert_eq!(fetched.1, bytes);
    let none: Value = ok(request(
        address,
        "POST",
        &format!("/v1/workspaces/{workspace}/blobs/missing"),
        Some(&device.token),
        Some(&json!({"hashes": [hash]})),
    ));
    assert_eq!(none["missing"], json!([]));
    let unauthorized = raw(address, "GET", &path, "zsd_wrong", None);
    assert_eq!(unauthorized.0, 401);
}
