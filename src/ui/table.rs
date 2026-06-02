//! Reusable virtualized results view for Quill.
//!
//! `DataTable` renders a [`QueryState`]: idle / loading / error / a tabular
//! result. The loaded result is drawn lazily via gpui's `uniform_list`, so a
//! 100k-row result only builds the rows currently on screen.
//!
//! Defined here in full (struct + constructor + state setter + render) before
//! any caller uses it. The owning view (`Workspace`) pushes new state in via
//! [`DataTable::set_state`].

use std::cmp::Ordering;
use std::sync::Arc;

use gpui::{
    canvas, div, prelude::*, px, rgb, uniform_list, Context, MouseButton, MouseMoveEvent,
    MouseUpEvent, SharedString, Window,
};

use crate::datasource::{QueryResult, QueryState};
use crate::ui::detail_format::{format_value, DetailFormat};
use crate::ui::theme;

/// A virtualized table view driven by a [`QueryState`].
///
/// A loaded result is held behind an `Arc` so the `uniform_list` row-builder
/// closure (which is `'static`) can cheaply clone a handle to it.
pub struct DataTable {
    state: QueryState,
    /// Cached `Arc` of the loaded result so the row closure can clone it
    /// without re-wrapping every frame.
    loaded: Option<Arc<QueryResult>>,
    /// The currently selected cell as `(display_row, col)`, if any. Drives the
    /// highlight and the detail panel. Row index is in *display* space (after
    /// sorting), so it's read through `order`.
    selected: Option<(usize, usize)>,
    /// Horizontal scroll offset in px (content shifts left by this). Manual,
    /// because `uniform_list` must NOT have an `overflow_x_scroll` ancestor (it
    /// collapses the list's height to zero). So we translate header+rows by a
    /// negative margin instead.
    scroll_x: f32,
    /// Real viewport width, measured each frame via a `canvas` (the only
    /// reliable way here). Drives scrollbar geometry + scroll clamping.
    viewport_w: f32,
    /// Grab offset inside the scrollbar thumb while dragging (px from thumb left).
    hbar_grab: Option<f32>,
    /// Current sort: `(col, ascending)`. `None` = original row order.
    sort: Option<(usize, bool)>,
    /// Display-row → original-row index. Identity when unsorted; recomputed on
    /// sort change. Always kept the same length as the result's row count.
    order: Vec<usize>,
    /// Height of the detail panel in px (user-draggable).
    detail_height: f32,
    /// Which transform the detail panel applies to the selected value.
    detail_format: DetailFormat,
    /// Drag state for the detail panel's resize handle: `(start_mouse_y,
    /// start_height)` while dragging.
    detail_drag: Option<(f32, f32)>,
}

impl DataTable {
    pub fn new() -> Self {
        Self {
            state: QueryState::Idle,
            loaded: None,
            selected: None,
            scroll_x: 0.0,
            viewport_w: 0.0,
            hbar_grab: None,
            sort: None,
            order: Vec::new(),
            detail_height: 200.0,
            detail_format: DetailFormat::Raw,
            detail_drag: None,
        }
    }

    /// Replace the current state and refresh. Called by the owning view inside
    /// `entity.update(cx, |this, cx| { this.set_state(...); cx.notify() })`.
    pub fn set_state(&mut self, state: QueryState) {
        self.loaded = match &state {
            QueryState::Loaded(r) => Some(Arc::new(r.clone())),
            _ => None,
        };
        // Reset view state — its coordinates refer to the previous result.
        self.selected = None;
        self.scroll_x = 0.0;
        self.sort = None;
        self.detail_format = DetailFormat::Raw;
        self.rebuild_order();
        self.state = state;
    }

    /// Rebuild `order` from the current `sort` against the loaded result.
    fn rebuild_order(&mut self) {
        let n = self.loaded.as_ref().map(|r| r.row_count()).unwrap_or(0);
        let mut order: Vec<usize> = (0..n).collect();
        if let (Some((col, ascending)), Some(result)) = (self.sort, self.loaded.as_ref()) {
            order.sort_by(|&a, &b| {
                let va = result.rows[a].cells.get(col).and_then(|c| c.as_ref());
                let vb = result.rows[b].cells.get(col).and_then(|c| c.as_ref());
                cmp_cells(va, vb, ascending)
            });
        }
        self.order = order;
    }

    /// The sticky header row. Shifted left by `scroll_x` to match the body, so
    /// they scroll horizontally in lockstep. Clicking a column header sorts.
    fn render_header(
        &self,
        result: &QueryResult,
        total_width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let cols = result.columns.clone();
        let sort = self.sort;
        div()
            .flex()
            .flex_row()
            .flex_none()
            .w(px(total_width))
            .h(px(theme::HEADER_HEIGHT))
            .bg(rgb(theme::SURFACE))
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            .children(cols.into_iter().enumerate().map(|(col_ix, c)| {
                // Sort indicator for the active column.
                let arrow = match sort {
                    Some((c, true)) if c == col_ix => " ▲",
                    Some((c, false)) if c == col_ix => " ▼",
                    _ => "",
                };
                div()
                    .id(("hdr", col_ix))
                    .w(px(theme::COL_WIDTH))
                    .flex_none()
                    .px(px(theme::PAD))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(1.))
                    .overflow_hidden()
                    .border_r_1()
                    .border_color(rgb(theme::BORDER))
                    .hover(|s| s.bg(rgb(theme::HOVER)))
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.toggle_sort(col_ix);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .font_family(theme::FONT_UI)
                            .text_color(rgb(theme::TEXT))
                            .text_size(px(theme::TEXT_SIZE_SM))
                            .child(SharedString::from(format!("{}{arrow}", c.name))),
                    )
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .font_family(theme::FONT_UI)
                            .text_color(rgb(theme::TEXT_DIM))
                            .text_size(px(theme::TEXT_SIZE_XS))
                            .child(SharedString::from(c.type_name)),
                    )
            }))
    }

    /// Cycle the sort for a column: none → asc → desc → none. Clears selection
    /// (its display-row index would otherwise point at the wrong row).
    fn toggle_sort(&mut self, col: usize) {
        self.sort = match self.sort {
            Some((c, true)) if c == col => Some((col, false)),
            Some((c, false)) if c == col => None,
            _ => Some((col, true)),
        };
        self.selected = None;
        self.rebuild_order();
    }

    /// The virtualized body. Only on-screen rows are built each frame.
    ///
    /// The row closure is `'static`, so it cannot borrow `&self`: the current
    /// `selected` cell is copied in by value (it's `Copy`), and clicks mutate
    /// state through a captured `Entity<Self>` rather than `cx.listener`.
    fn render_body(
        &self,
        result: Arc<QueryResult>,
        total_width: f32,
        selected: Option<(usize, usize)>,
        order: Arc<Vec<usize>>,
        entity: gpui::Entity<Self>,
    ) -> impl IntoElement {
        let row_count = order.len();
        let col_count = result.col_count();

        uniform_list("quill-rows", row_count, move |range, _window, _cx| {
            let result = result.clone();
            let order = order.clone();
            let entity = entity.clone();
            range
                .map(|display_ix| {
                    // Map display position → original row via the sort order.
                    let orig_ix = order[display_ix];
                    let row = &result.rows[orig_ix];
                    let bg = if display_ix % 2 == 0 {
                        theme::BG_PANEL
                    } else {
                        theme::ROW_ALT
                    };

                    let row_entity = entity.clone();
                    let cells = (0..col_count).map(move |col_ix| {
                        let value = row.cells.get(col_ix).and_then(|c| c.as_ref());
                        let is_selected = selected == Some((display_ix, col_ix));
                        // Fixed-width cell; clip overflow to one line with an
                        // ellipsis so long values never bleed into neighbors.
                        let mut cell = div()
                            .id(("cell", display_ix * col_count + col_ix))
                            .w(px(theme::COL_WIDTH))
                            .flex_none()
                            .px(px(theme::PAD))
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .border_r_1()
                            .border_color(rgb(theme::BORDER))
                            .font_family(theme::FONT_MONO)
                            .text_size(px(theme::TEXT_SIZE_SM))
                            .on_click({
                                let entity = row_entity.clone();
                                move |_ev, _window, cx| {
                                    entity.update(cx, |this, cx| {
                                        this.selected = Some((display_ix, col_ix));
                                        cx.notify();
                                    });
                                }
                            });
                        if is_selected {
                            cell = cell.bg(rgb(theme::SELECTED)).border_color(rgb(theme::ACCENT));
                        }
                        cell = match value {
                            Some(v) => cell.text_color(rgb(theme::TEXT)).child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .child(SharedString::from(v.clone())),
                            ),
                            // NULL: dim + italic to read as "absent", not data.
                            None => cell
                                .text_color(rgb(theme::TEXT_DIM))
                                .italic()
                                .child(SharedString::from("NULL")),
                        };
                        cell
                    });

                    div()
                        .id(display_ix)
                        .flex()
                        .flex_row()
                        .w(px(total_width))
                        .h(px(theme::ROW_HEIGHT))
                        .bg(rgb(bg))
                        .border_b_1()
                        .border_color(rgb(theme::BORDER))
                        .hover(|s| s.bg(rgb(theme::HOVER)))
                        .children(cells)
                })
                .collect::<Vec<_>>()
        })
        .w(px(total_width))
        .flex_grow()
        .min_h_0()
    }

    /// Footer with a row/column summary, aligned to the right.
    fn render_footer(&self, result: &QueryResult) -> impl IntoElement {
        let summary = format!("{} rows × {} cols", result.row_count(), result.col_count());
        div()
            .h(px(theme::ROW_HEIGHT))
            .px(px(theme::PAD_LG))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .bg(rgb(theme::SURFACE))
            .border_t_1()
            .border_color(rgb(theme::BORDER))
            .text_color(rgb(theme::TEXT_DIM))
            .text_size(px(theme::TEXT_SIZE_XS))
            .child(SharedString::from("Result"))
            .child(SharedString::from(summary))
    }

    /// A centered status message for the non-loaded states.
    fn render_message(&self, text: impl Into<SharedString>, color: u32) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .justify_center()
            .items_center()
            .text_color(rgb(color))
            .text_size(px(theme::TEXT_SIZE))
            .child(text.into())
    }

    /// A draggable horizontal scrollbar, shown when the grid overflows the
    /// measured viewport. Reads/writes the manual `scroll_x` so it stays in
    /// sync with trackpad scrolling. The thumb's mouse handlers are registered
    /// via a `canvas` (like gpui's `examples/data_table.rs`) so they get the
    /// thumb's real painted bounds.
    fn render_hscrollbar(&self, total_width: f32, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let track_w = self.viewport_w;
        // Need a measured viewport and an overflowing grid.
        if track_w <= 0.0 || total_width <= track_w + 1.0 {
            return None;
        }
        let max_scroll = (total_width - track_w).max(1.0);
        let thumb_w = (track_w * track_w / total_width).max(40.0);
        let travel = (track_w - thumb_w).max(0.0);
        let thumb_x = (self.scroll_x / max_scroll).clamp(0.0, 1.0) * travel;

        let entity = cx.entity();

        Some(
            div()
                .id("hbar")
                .relative()
                .h(px(12.))
                .w_full()
                .flex_none()
                .bg(rgb(theme::BG_DEEP))
                .border_t_1()
                .border_color(rgb(theme::BORDER))
                .child(
                    div()
                        .absolute()
                        .left(px(thumb_x))
                        .top(px(2.))
                        .h(px(8.))
                        .w(px(thumb_w))
                        .rounded(px(4.))
                        .bg(rgb(theme::ACCENT_DIM))
                        .hover(|s| s.bg(rgb(theme::ACCENT)))
                        .child(
                            canvas(
                                |_, _, _| (),
                                move |thumb_bounds, _, window, _| {
                                    window.on_mouse_event({
                                        let entity = entity.clone();
                                        move |ev: &gpui::MouseDownEvent, _, _, cx| {
                                            if thumb_bounds.contains(&ev.position) {
                                                let grab = f32::from(
                                                    ev.position.x - thumb_bounds.origin.x,
                                                );
                                                entity.update(cx, |this, _| {
                                                    this.hbar_grab = Some(grab);
                                                });
                                            }
                                        }
                                    });
                                    window.on_mouse_event({
                                        let entity = entity.clone();
                                        move |_: &MouseUpEvent, _, _, cx| {
                                            entity.update(cx, |this, _| this.hbar_grab = None);
                                        }
                                    });
                                    window.on_mouse_event({
                                        let entity = entity.clone();
                                        move |ev: &MouseMoveEvent, _, _, cx| {
                                            if !ev.dragging() {
                                                return;
                                            }
                                            let Some(grab) = entity.read(cx).hbar_grab else {
                                                return;
                                            };
                                            // Track-left = current thumb-left − its offset.
                                            let track_left =
                                                f32::from(thumb_bounds.origin.x) - thumb_x;
                                            let new_thumb_x =
                                                f32::from(ev.position.x) - track_left - grab;
                                            let frac = if travel > 0.0 {
                                                (new_thumb_x / travel).clamp(0.0, 1.0)
                                            } else {
                                                0.0
                                            };
                                            entity.update(cx, |this, cx| {
                                                this.scroll_x = frac * max_scroll;
                                                cx.notify();
                                            });
                                        }
                                    });
                                },
                            )
                            .size_full(),
                        ),
                ),
        )
    }

    /// One format toggle button in the detail toolbar.
    fn format_button(
        &self,
        fmt: DetailFormat,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.detail_format == fmt;
        div()
            .id(("fmt", fmt as usize))
            .px(px(theme::PAD_SM))
            .py(px(1.))
            .rounded(px(theme::RADIUS_SM))
            .font_family(theme::FONT_UI)
            .text_size(px(theme::TEXT_SIZE_XS))
            .bg(rgb(if active { theme::ACCENT } else { theme::SURFACE }))
            .text_color(rgb(if active { theme::BG_DEEP } else { theme::TEXT_DIM }))
            .hover(|s| s.text_color(rgb(theme::TEXT)))
            .on_click(cx.listener(move |this, _ev, _window, cx| {
                this.detail_format = fmt;
                cx.notify();
            }))
            .child(label)
    }

    /// Draggable detail panel showing the selected cell's full value, with a
    /// format toolbar (Raw/JSON/Base64/URL/Time). `None` when nothing is
    /// selected. Height is user-adjustable via the top drag handle.
    fn render_detail(&self, result: &QueryResult, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (display_row, c) = self.selected?;
        let col = result.columns.get(c)?;
        // Selection row is in display space → map to the original row.
        let orig_row = *self.order.get(display_row)?;
        let raw = result
            .rows
            .get(orig_row)
            .and_then(|row| row.cells.get(c))
            .map(|opt| opt.clone().unwrap_or_else(|| "NULL".into()))
            .unwrap_or_default();
        let shown = format_value(&raw, self.detail_format);

        Some(
            div()
                .flex_none()
                .w_full()
                .h(px(self.detail_height))
                .flex()
                .flex_col()
                .bg(rgb(theme::BG_PANEL))
                .border_t_1()
                .border_color(rgb(theme::BORDER))
                // Drag handle (top): adjusts detail_height.
                .child(
                    div()
                        .id("detail-resize")
                        .h(px(5.))
                        .w_full()
                        .bg(rgb(theme::BORDER))
                        .cursor(gpui::CursorStyle::ResizeUpDown)
                        .hover(|s| s.bg(rgb(theme::ACCENT)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, ev: &gpui::MouseDownEvent, _w, _cx| {
                                this.detail_drag =
                                    Some((f32::from(ev.position.y), this.detail_height));
                            }),
                        )
                        .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _w, cx| {
                            if let Some((start_y, start_h)) = this.detail_drag {
                                // Dragging up (smaller y) grows the panel.
                                let dy = start_y - f32::from(ev.position.y);
                                this.detail_height = (start_h + dy).clamp(80.0, 600.0);
                                cx.notify();
                            }
                        }))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, _cx| this.detail_drag = None),
                        ),
                )
                // Toolbar: column name + format buttons.
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(theme::PAD_SM))
                        .px(px(theme::PAD_LG))
                        .py(px(theme::PAD_SM))
                        .bg(rgb(theme::SURFACE))
                        .border_b_1()
                        .border_color(rgb(theme::BORDER))
                        .child(
                            div()
                                .flex_grow()
                                .font_family(theme::FONT_UI)
                                .text_size(px(theme::TEXT_SIZE_XS))
                                .text_color(rgb(theme::TEXT_DIM))
                                .child(SharedString::from(format!(
                                    "{}  ·  row {}",
                                    col.name,
                                    display_row + 1
                                ))),
                        )
                        .child(self.format_button(DetailFormat::Raw, "Raw", cx))
                        .child(self.format_button(DetailFormat::Json, "JSON", cx))
                        .child(self.format_button(DetailFormat::Base64, "Base64", cx))
                        .child(self.format_button(DetailFormat::Url, "URL", cx))
                        .child(self.format_button(DetailFormat::Timestamp, "Time", cx)),
                )
                // Value (transformed), monospace, scrollable.
                .child(
                    div()
                        .id("detail-value")
                        .flex_grow()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p(px(theme::PAD_LG))
                        .font_family(theme::FONT_MONO)
                        .text_size(px(theme::TEXT_SIZE_SM))
                        .text_color(rgb(theme::TEXT))
                        .child(SharedString::from(shown)),
                ),
        )
    }
}

impl Render for DataTable {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let base = div().size_full().flex().flex_col().bg(rgb(theme::BG_PANEL));

        match (&self.state, self.loaded.clone()) {
            (QueryState::Loaded(result), Some(arc)) => {
                // MANUAL horizontal scroll. `uniform_list` must NOT sit under an
                // `overflow_x_scroll` ancestor — that collapses its height to
                // zero and renders no rows (a bug hit repeatedly). So the
                // viewport is `overflow_hidden`, the body keeps a normal
                // flex_grow+min_h_0 height (uniform_list virtualizes vertically),
                // and we shift header+rows left by `scroll_x` via negative
                // margin. A `canvas` measures the real viewport width for
                // scrollbar geometry + clamping. Three inputs set scroll_x:
                // trackpad horizontal, Shift+wheel, and the scrollbar drag.
                let total_width = theme::COL_WIDTH * result.col_count().max(1) as f32;
                let scroll_x = self.scroll_x;
                let selected = self.selected;
                let order = Arc::new(self.order.clone());
                let entity = cx.entity();
                let measure_entity = cx.entity();

                // The wide content (header + rows) is ABSOLUTELY positioned inside
                // the viewport. Absolute elements are removed from layout flow, so
                // the 2560px content can NEVER stretch the viewport — the viewport
                // sizes purely to its flex slot (the visible width). We then shift
                // the content with `left(-scroll_x)` to scroll horizontally, and a
                // full-bleed `canvas` measures the true visible width.
                let content = div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(-scroll_x))
                    .w(px(total_width))
                    .flex()
                    .flex_col()
                    .child(self.render_header(result, total_width, cx))
                    .child(self.render_body(arc, total_width, selected, order, entity));

                let measure = canvas(
                    move |bounds, _, cx| {
                        let w = f32::from(bounds.size.width);
                        measure_entity.update(cx, |this, _| {
                            if (this.viewport_w - w).abs() > 0.5 {
                                this.viewport_w = w;
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0();

                let viewport = div()
                    .id("results-viewport")
                    .relative()
                    .flex_grow()
                    .min_h_0()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(move |this, ev: &gpui::ScrollWheelEvent, window, cx| {
                        let d = ev.delta.pixel_delta(window.line_height());
                        let dx = f32::from(d.x);
                        let dy = f32::from(d.y);
                        let amount = if ev.modifiers.shift {
                            if dx.abs() > dy.abs() { dx } else { dy }
                        } else if dx.abs() > dy.abs() {
                            dx
                        } else {
                            0.0
                        };
                        if amount != 0.0 {
                            let max = (total_width - this.viewport_w).max(0.0);
                            this.scroll_x = (this.scroll_x - amount).clamp(0.0, max);
                            cx.notify();
                        }
                    }))
                    .child(measure)
                    .child(content);

                base.child(viewport)
                    .children(self.render_hscrollbar(total_width, cx))
                    .children(self.render_detail(result, cx))
                    .child(self.render_footer(result))
            }
            (QueryState::Loading(what), _) => {
                base.child(self.render_message(format!("running… {what}"), theme::TEXT_DIM))
            }
            (QueryState::Error(e), _) => {
                base.child(self.render_message(format!("error: {e}"), theme::DANGER))
            }
            _ => base
                .child(self.render_message("Select a table or run a query", theme::TEXT_DIM)),
        }
    }
}

/// Compare two cells for sorting. NULLs sort last in *both* directions. For
/// two present values: numeric compare if both parse as f64, else string
/// compare; `ascending` only flips the Some/Some result.
fn cmp_cells(a: Option<&String>, b: Option<&String>, ascending: bool) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater, // null last
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => {
            let base = match (x.parse::<f64>(), y.parse::<f64>()) {
                (Ok(nx), Ok(ny)) => nx.partial_cmp(&ny).unwrap_or(Ordering::Equal),
                _ => x.cmp(y),
            };
            if ascending {
                base
            } else {
                base.reverse()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datasource::{Column, Row};

    fn one_row_result() -> QueryResult {
        QueryResult {
            columns: vec![Column { name: "id".into(), type_name: "INT".into() }],
            rows: vec![Row { cells: vec![Some("1".into())] }],
        }
    }

    #[test]
    fn set_state_caches_arc_only_when_loaded() {
        let mut t = DataTable::new();
        assert!(t.loaded.is_none());

        t.set_state(QueryState::Loading("x".into()));
        assert!(t.loaded.is_none(), "loading must not cache a result");

        t.set_state(QueryState::Loaded(one_row_result()));
        assert!(t.loaded.is_some(), "loaded must cache the Arc for the row closure");

        t.set_state(QueryState::Error("boom".into()));
        assert!(t.loaded.is_none(), "error must clear the cached result");
    }
}
