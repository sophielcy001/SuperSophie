// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reads a web page's title and description for a bookmark card. Runs only when the
//! user presses 「读取网页标题」; nothing is fetched on paste or when a page opens.

use reqwest::{Client, header, redirect};
use serde::Serialize;
use std::time::Duration;
use tauri::AppHandle;
use zhiye_core::{Error, Result, image_extension, web_url};

/// Enough for the `<head>` of practically every page.
const BODY_LIMIT: usize = 512 * 1024;
const TITLE_LIMIT: usize = 500;
const DESCRIPTION_LIMIT: usize = 1000;
/// Preview images larger than this are skipped.
const IMAGE_LIMIT: usize = 5 * 1024 * 1024;

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct WebSummary {
    pub title: Option<String>,
    pub description: Option<String>,
    /// The page's `og:image`, saved as an attachment of the bookmark's page.
    pub image: Option<PreviewImage>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewImage {
    pub attachment_id: String,
    pub media_type: String,
}

/// Reads the title and description; with a workspace and page, also saves the preview
/// image there, so the card shows it later without going online.
#[tauri::command]
pub async fn fetch_web_title(
    app: AppHandle,
    href: String,
    workspace: Option<String>,
    page: Option<String>,
) -> Result<WebSummary> {
    let (mut summary, image) = read_page(&href).await?;
    if let (Some(workspace), Some(page), Some((media_type, bytes))) = (workspace, page, image) {
        let extension = image_extension(&media_type).unwrap_or("png");
        let name = format!("书签预览.{extension}");
        let saved = crate::run(app, move |store| {
            store.add_image(&workspace, &page, &name, bytes.as_slice())
        })
        .await?;
        summary.image = Some(PreviewImage {
            attachment_id: saved.attachment.id,
            media_type,
        });
    }
    Ok(summary)
}

/// Fetches the page: its summary, and the bytes and type of its preview image if it has one.
async fn read_page(href: &str) -> Result<(WebSummary, Option<(String, Vec<u8>)>)> {
    let url = web_url(href)
        .ok_or_else(|| Error::new("INVALID_URL", "链接需要完整的 HTTP 或 HTTPS 地址。"))?;
    let failed =
        |error: reqwest::Error| Error::new("WEB_TITLE", format!("无法读取网页标题。{error}"));
    let client = Client::builder()
        // Generous enough for slow networks; the card shows its address meanwhile.
        .timeout(Duration::from_secs(20))
        .redirect(redirect::Policy::limited(5))
        .user_agent("Zhiye bookmark title")
        .build()
        .map_err(failed)?;
    let mut response = client
        .get(&url)
        .header(header::ACCEPT, "text/html,application/xhtml+xml")
        .send()
        .await
        .map_err(failed)?;
    if !response.status().is_success() {
        return Ok((WebSummary::default(), None));
    }
    let base = response.url().clone();
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => body.extend_from_slice(&chunk),
            Ok(None) => break,
            // A slow or broken transfer keeps what already arrived: the title is near the top.
            Err(_) if !body.is_empty() => break,
            Err(error) => return Err(failed(error)),
        }
        // Everything a bookmark needs is in <head>; stop there, as large pages can be slow.
        if body.len() >= BODY_LIMIT || contains_head_end(&body) {
            break;
        }
    }
    let html = String::from_utf8_lossy(&body);
    let summary = WebSummary {
        title: page_title(&html),
        description: meta_content(
            &html,
            &[
                "og:description",
                "name=\"description\"",
                "name='description'",
            ],
        )
        .and_then(|text| clean(text, DESCRIPTION_LIMIT)),
        image: None,
    };
    let image_url = meta_content(&html, &["og:image"])
        .map(decode_entities)
        .and_then(|address| base.join(address.trim()).ok())
        .and_then(|address| web_url(address.as_str()));
    let image = match image_url {
        Some(address) => preview_image(&client, &address).await,
        None => None,
    };
    Ok((summary, image))
}

fn contains_head_end(body: &[u8]) -> bool {
    body.windows(7)
        .any(|window| window.eq_ignore_ascii_case(b"</head>"))
}

/// Downloads a preview image; anything that is not a supported image within the limit is skipped.
async fn preview_image(client: &Client, address: &str) -> Option<(String, Vec<u8>)> {
    let mut response = client
        .get(address)
        .header(header::ACCEPT, "image/*")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let media_type = response
        .headers()
        .get(header::CONTENT_TYPE)?
        .to_str()
        .ok()?
        .split(';')
        .next()?
        .trim()
        .to_ascii_lowercase();
    image_extension(&media_type)?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > IMAGE_LIMIT {
            return None;
        }
    }
    (!bytes.is_empty()).then_some((media_type, bytes))
}

/// The page's `<title>`, or its `og:title` when the title element is missing or empty.
pub fn page_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let from_title = lower.find("<title").and_then(|start| {
        let open = start + lower[start..].find('>')? + 1;
        let close = open + lower[open..].find("</title")?;
        clean(&html[open..close], TITLE_LIMIT)
    });
    from_title
        .or_else(|| meta_content(html, &["og:title"]).and_then(|text| clean(text, TITLE_LIMIT)))
}

/// The `content` of the first `<meta>` tag mentioning one of the markers, in marker order.
fn meta_content<'a>(html: &'a str, markers: &[&str]) -> Option<&'a str> {
    let lower = html.to_ascii_lowercase();
    markers.iter().find_map(|marker| {
        let mut rest = 0;
        while let Some(found) = lower[rest..].find("<meta") {
            let start = rest + found;
            let end = start + lower[start..].find('>')?;
            let tag_lower = &lower[start..end];
            if tag_lower.contains(marker)
                && let Some(content) = attribute(&html[start..end], tag_lower, "content")
            {
                return Some(content);
            }
            rest = end;
        }
        None
    })
}

fn attribute<'a>(tag: &'a str, lower: &str, name: &str) -> Option<&'a str> {
    let at = lower.find(&format!("{name}="))? + name.len() + 1;
    let quote = tag[at..].chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = &tag[at + 1..];
    Some(&value[..value.find(quote)?])
}

fn clean(raw: &str, limit: usize) -> Option<String> {
    let decoded = decode_entities(raw);
    let text: String = decoded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect();
    (!text.is_empty()).then_some(text)
}

fn decode_entities(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        output.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            output.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let character = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| {
                    entity
                        .strip_prefix('#')
                        .and_then(|decimal| decimal.parse().ok())
                })
                .and_then(char::from_u32),
        };
        match character {
            Some(character) => {
                output.push(character);
                rest = &rest[end + 1..];
            }
            None => {
                output.push('&');
                rest = &rest[1..];
            }
        }
    }
    output.push_str(rest);
    output
}

#[cfg(test)]
mod tests {
    use super::{WebSummary, page_title, read_page};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    #[ignore = "reaches the internet"]
    fn reads_a_real_page() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let started = std::time::Instant::now();
        let result = runtime.block_on(read_page("https://github.com/"));
        match &result {
            Ok((summary, image)) => println!(
                "{:?} title={:?} image={:?}",
                started.elapsed(),
                summary.title,
                image
                    .as_ref()
                    .map(|(kind, bytes)| (kind.clone(), bytes.len()))
            ),
            Err(error) => println!("{:?} error={error:?}", started.elapsed()),
        }
    }

    /// Fetches from a real HTTP server on this machine, so the test needs no internet.
    #[test]
    fn fetches_title_description_and_preview_image_over_http() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let png: &[u8] = b"\x89PNG\r\n\x1a\n preview";
        let server = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 2048];
                let read = stream.read(&mut request).unwrap();
                let line = String::from_utf8_lossy(&request[..read])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let (status, kind, body): (&str, &str, Vec<u8>) = if line.contains("/cover.png") {
                    ("200 OK", "image/png", png.to_vec())
                } else if line.contains("/missing") {
                    ("404 Not Found", "text/html", b"gone".to_vec())
                } else {
                    ("200 OK", "text/html; charset=utf-8", "<html><head><title>本地 &amp; 网页</title>\
                        <meta name=\"description\" content=\"用于书签卡片的 摘要\">\
                        <meta property=\"og:image\" content=\"/cover.png\"></head><body>正文</body></html>".as_bytes().to_vec())
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (summary, image) = runtime
            .block_on(read_page(&format!("http://{address}/page")))
            .unwrap();
        assert_eq!(
            summary,
            WebSummary {
                title: Some("本地 & 网页".into()),
                description: Some("用于书签卡片的 摘要".into()),
                image: None,
            }
        );
        assert_eq!(image, Some(("image/png".into(), png.to_vec())));
        let (missing, none) = runtime
            .block_on(read_page(&format!("http://{address}/missing")))
            .unwrap();
        assert_eq!((missing, none), (WebSummary::default(), None));
        assert_eq!(
            runtime
                .block_on(read_page("file:///etc/hosts"))
                .unwrap_err()
                .code,
            "INVALID_URL"
        );
        server.join().unwrap();
    }

    #[test]
    fn reads_title_then_open_graph_and_decodes_entities() {
        assert_eq!(
            page_title("<html><head><TITLE lang=zh>\n 知页 &amp; Notion&#x3002;</TITLE></head>")
                .as_deref(),
            Some("知页 & Notion。")
        );
        assert_eq!(
            page_title(r#"<head><title> </title><meta property="og:title" content="备用 &quot;标题&quot;"></head>"#)
                .as_deref(),
            Some("备用 \"标题\"")
        );
        assert_eq!(page_title("<body>没有标题</body>"), None);
        assert_eq!(
            page_title("<title>&bogus; 5 &lt; 6</title>").as_deref(),
            Some("&bogus; 5 < 6")
        );
        assert_eq!(
            page_title(&format!("<title>{}</title>", "长".repeat(800)))
                .map(|title| title.chars().count()),
            Some(500)
        );
    }
}
