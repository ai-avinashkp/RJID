//! Custom scroll-position thumb.
//!
//! GPUI's Windows backend provides `overflow_scroll`'s scrolling *mechanics*
//! (mouse wheel, drag) but does not paint a native scrollbar thumb — so
//! without this, a scrollable panel has no visual indication of scroll
//! position at all on Windows. This draws a thin (5px) overlay indicator
//! computed from a `ScrollHandle`'s current/max offset; it is visual only
//! (not draggable) — a fast-follow if click-to-scroll-here or dragging the
//! thumb itself is wanted later.

use gpui::{IntoElement, Rgba, ScrollHandle, div, prelude::*, px};

const THUMB_MIN_HEIGHT: f32 = 24.0;
const THUMB_MIN_WIDTH: f32 = 24.0;
/// Thickness of the thumb along the scrollbar's short axis (both the
/// vertical thumb's width and the horizontal thumb's height).
const THUMB_WIDTH: f32 = 10.0;

/// Builds a thumb overlay for a vertical scroll area, or `None` if the
/// content isn't taller than the viewport (nothing to scroll, nothing to
/// show). `viewport_h` is the scrollable container's own height in pixels.
/// The caller must wrap the scrollable content and this thumb together in a
/// `.relative()` parent so the thumb's `.absolute()` positioning anchors to
/// the right place.
pub fn scroll_thumb(
    handle: &ScrollHandle,
    viewport_h: f32,
    color: Rgba,
) -> Option<impl IntoElement + use<>> {
    let max_offset_y = f32::from(handle.max_offset().y);
    if max_offset_y <= 0.0 {
        return None;
    }

    let content_h = viewport_h + max_offset_y;
    let thumb_h = (viewport_h * (viewport_h / content_h)).clamp(THUMB_MIN_HEIGHT, viewport_h);
    let scrolled = f32::from(-handle.offset().y).clamp(0.0, max_offset_y);
    let progress = scrolled / max_offset_y;
    let thumb_top = progress * (viewport_h - thumb_h);

    Some(
        div()
            .absolute()
            .top(px(thumb_top))
            .right(px(1.))
            .w(px(THUMB_WIDTH))
            .h(px(thumb_h))
            .rounded_sm()
            .bg(color),
    )
}

/// Same idea as [`scroll_thumb`], but for horizontal scrolling (long lines)
/// — anchored along the bottom edge instead of the right edge.
pub fn scroll_thumb_horizontal(
    handle: &ScrollHandle,
    viewport_w: f32,
    color: Rgba,
) -> Option<impl IntoElement + use<>> {
    let max_offset_x = f32::from(handle.max_offset().x);
    if max_offset_x <= 0.0 {
        return None;
    }

    let content_w = viewport_w + max_offset_x;
    let thumb_w = (viewport_w * (viewport_w / content_w)).clamp(THUMB_MIN_WIDTH, viewport_w);
    let scrolled = f32::from(-handle.offset().x).clamp(0.0, max_offset_x);
    let progress = scrolled / max_offset_x;
    let thumb_left = progress * (viewport_w - thumb_w);

    Some(
        div()
            .absolute()
            .left(px(thumb_left))
            .bottom(px(1.))
            .h(px(THUMB_WIDTH))
            .w(px(thumb_w))
            .rounded_sm()
            .bg(color),
    )
}
