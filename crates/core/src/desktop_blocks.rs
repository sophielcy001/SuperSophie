// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::{Block, Error, Result};
use serde_json::{Value, json};

const OPAQUE: &str = "$zhiyeOpaque";
const JS_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

pub fn encode_desktop_blocks(mut blocks: Vec<Block>) -> Result<Vec<Block>> {
    for block in &mut blocks {
        if block.extra.contains_key(OPAQUE)
            || requires_opaque(&block.payload)
            || block.extra.values().any(requires_opaque)
        {
            let original = serde_json::to_string(block)?;
            block.payload = json!({});
            block.children.clear();
            block.extra.clear();
            block.extra.insert(OPAQUE.into(), Value::String(original));
        } else {
            block.children = encode_desktop_blocks(std::mem::take(&mut block.children))?;
        }
    }
    Ok(blocks)
}

pub fn decode_desktop_blocks(mut blocks: Vec<Block>) -> Result<Vec<Block>> {
    for block in &mut blocks {
        if let Some(original) = block.extra.remove(OPAQUE) {
            let raw = original
                .as_str()
                .ok_or_else(|| Error::new("INVALID_BLOCK", "保留块的内容格式无效。"))?;
            *block = serde_json::from_str(raw)?;
        } else {
            block.children = decode_desktop_blocks(std::mem::take(&mut block.children))?;
        }
    }
    Ok(blocks)
}

fn requires_opaque(value: &Value) -> bool {
    match value {
        Value::Number(number) => {
            // JavaScript also normalizes numeric representations when serializing JSON.
            number
                .as_i64()
                .is_none_or(|number| !(-JS_SAFE_INTEGER..=JS_SAFE_INTEGER).contains(&number))
        }
        Value::Array(values) => values.iter().any(requires_opaque),
        Value::Object(fields) => fields.values().any(requires_opaque),
        _ => false,
    }
}
