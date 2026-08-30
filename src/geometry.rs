//! Geometry helpers for element-scoped and downscaled screenshots: CDP box-model resolution
//! (uid / CSS selector → clip rectangle) and the pure math (bounding box, downscale factor).

use std::collections::HashMap;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{BoxModel, GetBoxModelResult, Quad};
use crate::element_ref::ElementRef;

/// An axis-aligned clip rectangle in page CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The axis-aligned bounding box of a CDP quad `[x1,y1, x2,y2, x3,y3, x4,y4]`. A malformed
/// (short) quad yields a zero rect rather than a panic.
#[must_use]
pub fn quad_bounds(quad: &Quad) -> Rect {
    if quad.len() < 8 {
        return Rect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        };
    }
    let xs = [quad[0], quad[2], quad[4], quad[6]];
    let ys = [quad[1], quad[3], quad[5], quad[7]];
    let min_x = xs.iter().copied().fold(f64::INFINITY, f64::min);
    let max_x = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min_y = ys.iter().copied().fold(f64::INFINITY, f64::min);
    let max_y = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Rect {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    }
}

/// Downscale factor so `width` fits within `max_width`; never upscales. A `None` cap, a
/// non-positive width, or a width already within bounds all yield `1.0`.
#[must_use]
pub fn compute_scale(width: f64, max_width: Option<u32>) -> f64 {
    match max_width {
        Some(max) if max > 0 && width > f64::from(max) => f64::from(max) / width,
        _ => 1.0,
    }
}

/// Border-box clip rectangle for an element identified by uid.
pub async fn clip_for_uid(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
) -> Result<Rect, crate::BoxError> {
    let element_ref = uid_map.get(uid).ok_or_else(|| {
        format!("Element uid={uid} not found. Run 'chrome-agent inspect' to get fresh uids.")
    })?;
    let backend_node_id = element_ref
        .backend_node_id()
        .ok_or_else(|| format!("Element uid={uid} has no resolvable backend node."))?;
    let result: GetBoxModelResult = client
        .call(
            "DOM.getBoxModel",
            json!({ "backendNodeId": backend_node_id }),
        )
        .await
        .map_err(|e| {
            format!("Element uid={uid} has no box model (not rendered / zero-size): {e}")
        })?;
    Ok(border_clip(&result.model))
}

/// Border-box clip rectangle for the first element matching a CSS selector.
pub async fn clip_for_selector(
    client: &CdpClient,
    selector: &str,
) -> Result<Rect, crate::BoxError> {
    let doc: serde_json::Value = client
        .call("DOM.getDocument", json!({ "depth": 0 }))
        .await?;
    let root_id = doc
        .get("root")
        .and_then(|r| r.get("nodeId"))
        .and_then(serde_json::Value::as_i64)
        .ok_or("DOM.getDocument returned no root nodeId")?;

    let found: serde_json::Value = client
        .call(
            "DOM.querySelector",
            json!({ "nodeId": root_id, "selector": selector }),
        )
        .await?;
    let node_id = found
        .get("nodeId")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    if node_id == 0 {
        return Err(format!("No element matches selector: {selector}").into());
    }

    let result: GetBoxModelResult = client
        .call("DOM.getBoxModel", json!({ "nodeId": node_id }))
        .await?;
    Ok(border_clip(&result.model))
}

fn border_clip(model: &BoxModel) -> Rect {
    quad_bounds(&model.border)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_bounds_axis_aligned() {
        // A 100x50 box at (10, 20): [x1,y1 .. x4,y4] clockwise.
        let quad = vec![10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0];
        let r = quad_bounds(&quad);
        assert_eq!(
            r,
            Rect {
                x: 10.0,
                y: 20.0,
                width: 100.0,
                height: 50.0
            }
        );
    }

    #[test]
    fn quad_bounds_unordered_points() {
        // Unordered points still give the min/max envelope.
        let quad = vec![110.0, 70.0, 10.0, 20.0, 110.0, 20.0, 10.0, 70.0];
        let r = quad_bounds(&quad);
        assert_eq!(
            r,
            Rect {
                x: 10.0,
                y: 20.0,
                width: 100.0,
                height: 50.0
            }
        );
    }

    #[test]
    fn quad_bounds_malformed_is_zero() {
        let r = quad_bounds(&vec![1.0, 2.0, 3.0]);
        assert_eq!(
            r,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0
            }
        );
    }

    #[test]
    fn scale_none_is_identity() {
        assert!((compute_scale(1600.0, None) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn scale_within_bounds_is_identity() {
        assert!((compute_scale(800.0, Some(1024)) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn scale_downscales_wide() {
        // 1600 → cap 800 = 0.5
        assert!((compute_scale(1600.0, Some(800)) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn scale_zero_cap_is_identity() {
        // A degenerate cap must not divide by zero or blank the image.
        assert!((compute_scale(1600.0, Some(0)) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn scale_zero_width_is_identity() {
        assert!((compute_scale(0.0, Some(800)) - 1.0).abs() < f64::EPSILON);
    }
}
