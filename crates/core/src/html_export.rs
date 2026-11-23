// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::{Attachment, PageLocation, web_url};
use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, html};
use std::collections::HashSet;

pub(crate) fn render_document(
    title: &str,
    markdown: &str,
    assets: &[(Attachment, String)],
) -> String {
    let asset_paths: HashSet<&str> = assets
        .iter()
        .map(|(_, path)| path.as_str())
        .filter(|path| safe_relative_path(path))
        .collect();
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_GFM;
    let events = Parser::new_ext(markdown, options).map(|event| match event {
        Event::InlineHtml(tag) if matches!(tag.as_ref(), "<u>" | "</u>" | "<br>") => {
            Event::InlineHtml(tag)
        }
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        mut event => {
            match &mut event {
                Event::Start(Tag::Image { dest_url, .. })
                    if !asset_paths.contains(dest_url.as_ref()) =>
                {
                    *dest_url = CowStr::Borrowed("#");
                }
                Event::Start(Tag::Link { dest_url, .. })
                    if !asset_paths.contains(dest_url.as_ref())
                        && PageLocation::parse(dest_url).is_err()
                        && web_url(dest_url).is_none() =>
                {
                    *dest_url = CowStr::Borrowed("#");
                }
                _ => {}
            }
            event
        }
    });

    let mut document = String::from(
        "<!doctype html>\n<html lang=\"zh-CN\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src 'self' file:; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"referrer\" content=\"no-referrer\">\n<title>",
    );
    for character in title.chars() {
        match character {
            '&' => document.push_str("&amp;"),
            '<' => document.push_str("&lt;"),
            '>' => document.push_str("&gt;"),
            _ => document.push(character),
        }
    }
    document.push_str("</title>\n<style>");
    document.push_str(STYLE);
    document.push_str("</style>\n</head>\n<body>\n<main>\n");
    html::push_html(&mut document, events);
    document.push_str("</main>\n</body>\n</html>\n");
    document
}

fn safe_relative_path(path: &str) -> bool {
    path.split('/').all(|part| {
        !matches!(part, "" | "." | "..")
            && part
                .bytes()
                .all(|character| character.is_ascii_alphanumeric() || b"-_.".contains(&character))
    })
}

const STYLE: &str = "
  :root { color-scheme: light dark; font-family: Inter, system-ui, sans-serif; line-height: 1.6; }
  body { margin: 0; background: Canvas; color: CanvasText; }
  main { box-sizing: border-box; max-inline-size: 78ch; margin: auto; padding: clamp(1rem, 5vw, 3rem); overflow-wrap: anywhere; }
  h1, h2, h3, h4 { line-height: 1.25; }
  img { max-inline-size: 100%; block-size: auto; }
  blockquote { margin-inline: 0; border-inline-start: 3px solid GrayText; padding-inline-start: 1rem; }
  pre { background: color-mix(in srgb, CanvasText 8%, Canvas); padding: 0.9rem; overflow: auto; }
  code { font-family: 'IBM Plex Mono', ui-monospace, monospace; }
  table { display: block; inline-size: max-content; max-inline-size: 100%; overflow: auto; border-collapse: collapse; }
  th, td { border: 1px solid GrayText; padding: 0.35rem 0.65rem; text-align: start; }
  @media print { main { max-inline-size: none; padding: 0; } }
";
