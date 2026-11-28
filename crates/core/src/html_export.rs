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
        Event::InlineHtml(tag)
            if matches!(tag.as_ref(), "<u>" | "</u>" | "<br>" | "</span>") || color_span(&tag) =>
        {
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

/// The only other inline HTML the Markdown renderer writes: a span carrying a Notion color.
fn color_span(tag: &str) -> bool {
    let Some((attribute, color)) = tag
        .strip_prefix("<span ")
        .and_then(|rest| rest.strip_suffix("\">"))
        .and_then(|rest| rest.split_once("=\""))
    else {
        return false;
    };
    match attribute {
        "data-text-color" | "data-background-color" => crate::rich_blocks::inline_color(color),
        "data-block-color" => crate::rich_blocks::block_color(color),
        _ => false,
    }
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
  /* Notion's text and background colors. */
  [data-text-color=gray], [data-block-color=gray] { color: #787774; }
  [data-background-color=gray], [data-block-color=gray_background] { background: #f1f1ef; border-radius: 3px; }
  [data-text-color=brown], [data-block-color=brown] { color: #9f6b53; }
  [data-background-color=brown], [data-block-color=brown_background] { background: #f4eeee; border-radius: 3px; }
  [data-text-color=orange], [data-block-color=orange] { color: #d9730d; }
  [data-background-color=orange], [data-block-color=orange_background] { background: #fbecdd; border-radius: 3px; }
  [data-text-color=yellow], [data-block-color=yellow] { color: #cb912f; }
  [data-background-color=yellow], [data-block-color=yellow_background] { background: #fbf3db; border-radius: 3px; }
  [data-text-color=green], [data-block-color=green] { color: #448361; }
  [data-background-color=green], [data-block-color=green_background] { background: #edf3ec; border-radius: 3px; }
  [data-text-color=blue], [data-block-color=blue] { color: #337ea9; }
  [data-background-color=blue], [data-block-color=blue_background] { background: #e7f3f8; border-radius: 3px; }
  [data-text-color=purple], [data-block-color=purple] { color: #9065b0; }
  [data-background-color=purple], [data-block-color=purple_background] { background: #f6f3f9; border-radius: 3px; }
  [data-text-color=pink], [data-block-color=pink] { color: #c14c8a; }
  [data-background-color=pink], [data-block-color=pink_background] { background: #faf1f5; border-radius: 3px; }
  [data-text-color=red], [data-block-color=red] { color: #d44c47; }
  [data-background-color=red], [data-block-color=red_background] { background: #fdebec; border-radius: 3px; }
  @media (prefers-color-scheme: dark) {
    [data-text-color=gray], [data-block-color=gray] { color: #9b9b9b; }
    [data-background-color=gray], [data-block-color=gray_background] { background: #2f2f2f; border-radius: 3px; }
    [data-text-color=brown], [data-block-color=brown] { color: #ba856f; }
    [data-background-color=brown], [data-block-color=brown_background] { background: #4a3228; border-radius: 3px; }
    [data-text-color=orange], [data-block-color=orange] { color: #c77d48; }
    [data-background-color=orange], [data-block-color=orange_background] { background: #5c3b23; border-radius: 3px; }
    [data-text-color=yellow], [data-block-color=yellow] { color: #ca9849; }
    [data-background-color=yellow], [data-block-color=yellow_background] { background: #564328; border-radius: 3px; }
    [data-text-color=green], [data-block-color=green] { color: #529e72; }
    [data-background-color=green], [data-block-color=green_background] { background: #243d30; border-radius: 3px; }
    [data-text-color=blue], [data-block-color=blue] { color: #379ad3; }
    [data-background-color=blue], [data-block-color=blue_background] { background: #143a4e; border-radius: 3px; }
    [data-text-color=purple], [data-block-color=purple] { color: #9d68d3; }
    [data-background-color=purple], [data-block-color=purple_background] { background: #3c2d49; border-radius: 3px; }
    [data-text-color=pink], [data-block-color=pink] { color: #d15796; }
    [data-background-color=pink], [data-block-color=pink_background] { background: #4e2c3c; border-radius: 3px; }
    [data-text-color=red], [data-block-color=red] { color: #df5452; }
    [data-background-color=red], [data-block-color=red_background] { background: #522e2a; border-radius: 3px; }
  }
";
