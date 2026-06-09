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
    canvas, div, prelude::*, px, rgb, uniform_list, Context, EventEmitter, Focusable,
    MouseButton, MouseMoveEvent, MouseUpEvent, SharedString, Window,
};

use crate::datasource::{QueryResult, QueryState};
use crate::ui::detail_format::{format_value, tokenize_json, DetailFormat};
use crate::ui::text_input::{InputEvent, TextInput};
use crate::ui::theme;

/// Events emitted by `DataTable` to the owning `Session`.
#[derive(Debug, Clone)]
pub enum TableEvent {
    /// User committed an inline cell edit; the session should execute an UPDATE.
    UpdateCell { orig_row: usize, col: usize, new_value: String },
}

impl EventEmitter<TableEvent> for DataTable {}

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
    /// Measured width of the detail value area, for soft-wrapping long text.
    detail_width: f32,
    /// Which transform the detail panel applies to the selected value.
    detail_format: DetailFormat,
    /// Drag state for the detail panel's resize handle: `(start_mouse_y,
    /// start_height)` while dragging.
    detail_drag: Option<(f32, f32)>,
    /// Whether the current result is editable (set by Session when the query
    /// was triggered by a sidebar table click, so the db+table are known).
    editable: bool,
    /// The cell currently being inline-edited as `(display_row, col)`.
    editing: Option<(usize, usize)>,
    /// Reusable text input widget for inline cell editing.
    edit_input: gpui::Entity<TextInput>,
}

impl DataTable {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let edit_input = cx.new(|cx| TextInput::new(cx, ""));
        // Commit edit on Enter.
        cx.subscribe(&edit_input, |this, _input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Submit) {
                this.commit_edit(cx);
            }
        })
        .detach();
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
            detail_width: 800.0,
            detail_format: DetailFormat::Raw,
            detail_drag: None,
            editable: false,
            editing: None,
            edit_input,
        }
    }

    /// Mark whether the current result allows inline editing. Called by Session
    /// after a table-click query (where db+table are known).
    pub fn set_editable(&mut self, editable: bool) {
        self.editable = editable;
    }

    /// Expose the loaded result so Session can read row data to build UPDATE SQL.
    pub fn loaded_result(&self) -> Option<Arc<QueryResult>> {
        self.loaded.clone()
    }

    /// Enter edit mode for a cell identified by *display* row + column.
    fn start_edit(&mut self, display_row: usize, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(result) = &self.loaded else { return };
        let orig_row = match self.order.get(display_row) { Some(&r) => r, None => return };
        let current = result.rows.get(orig_row)
            .and_then(|r| r.cells.get(col))
            .and_then(|c| c.as_ref())
            .cloned()
            .unwrap_or_default();

        self.editing = Some((display_row, col));
        self.selected = Some((display_row, col));
        self.edit_input.update(cx, |inp, cx| inp.set_content(current, cx));
        let focus = self.edit_input.read(cx).focus_handle(cx).clone();
        window.focus(&focus);
        cx.notify();
    }

    /// Cancel inline edit without committing.
    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        self.editing = None;
        cx.notify();
    }

    /// Commit the inline edit: read the input value, map display→orig row, emit event.
    fn commit_edit(&mut self, cx: &mut Context<Self>) {
        let Some((display_row, col)) = self.editing.take() else { return };
        let new_value = self.edit_input.read(cx).content().to_string();
        let orig_row = match self.order.get(display_row) { Some(&r) => r, None => return };
        cx.emit(TableEvent::UpdateCell { orig_row, col, new_value });
        cx.notify();
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
        self.editing = None;
        // Note: `editable` is NOT reset here — Session controls it explicitly via
        // set_editable() so it survives the async set_state call from run_query.
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
            // Leading row-number column header.
            .child(
                div()
                    .w(px(theme::SEQ_WIDTH))
                    .flex_none()
                    .px(px(theme::PAD))
                    .flex()
                    .items_center()
                    .justify_end()
                    .border_r_1()
                    .border_color(rgb(theme::BORDER))
                    .font_family(theme::FONT_UI)
                    .text_color(rgb(theme::TEXT_DIM))
                    .text_size(px(theme::TEXT_SIZE_XS))
                    .child("#"),
            )
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
        editing: Option<(usize, usize)>,
        edit_input: gpui::Entity<TextInput>,
        editable: bool,
        order: Arc<Vec<usize>>,
        entity: gpui::Entity<Self>,
    ) -> impl IntoElement {
        let row_count = order.len();
        let col_count = result.col_count();

        uniform_list("quill-rows", row_count, move |range, _window, _cx| {
            let result = result.clone();
            let order = order.clone();
            let entity = entity.clone();
            let edit_input = edit_input.clone();
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
                    let edit_input = edit_input.clone();
                    let cells = (0..col_count).map(move |col_ix| {
                        let value = row.cells.get(col_ix).and_then(|c| c.as_ref());
                        let is_selected = selected == Some((display_ix, col_ix));
                        let is_editing = editing == Some((display_ix, col_ix));

                        // Fixed-width cell; clip overflow to one line with an
                        // ellipsis so long values never bleed into neighbors.
                        let mut cell = div()
                            .id(("cell", display_ix * col_count + col_ix))
                            .w(px(theme::COL_WIDTH))
                            .flex_none()
                            .px(px(if is_editing { 0.0 } else { theme::PAD }))
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .border_r_1()
                            .border_color(rgb(theme::BORDER))
                            .font_family(theme::FONT_MONO)
                            .text_size(px(theme::TEXT_SIZE_SM))
                            .on_click({
                                let entity = row_entity.clone();
                                move |ev, window, cx| {
                                    entity.update(cx, |this, cx| {
                                        this.selected = Some((display_ix, col_ix));
                                        // Cancel any in-progress edit when clicking elsewhere.
                                        if this.editing.is_some()
                                            && this.editing != Some((display_ix, col_ix))
                                        {
                                            this.editing = None;
                                        }
                                        cx.notify();
                                    });
                                    // Double-click enters edit mode (editable tables only).
                                    if ev.click_count() == 2 {
                                        entity.update(cx, |this, cx| {
                                            if this.editable {
                                                this.start_edit(display_ix, col_ix, window, cx);
                                            }
                                        });
                                    }
                                }
                            });

                        if is_editing {
                            // Render the inline TextInput in place of the cell text.
                            cell = cell
                                .bg(rgb(theme::BG_DEEP))
                                .border_color(rgb(theme::ACCENT))
                                .child(edit_input.clone());
                        } else {
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
                            // Show pencil hint on hover for editable tables.
                            if editable && is_selected {
                                cell = cell.child(
                                    div()
                                        .flex_none()
                                        .pl(px(theme::PAD_XS))
                                        .text_color(rgb(theme::TEXT_DIM))
                                        .text_size(px(theme::TEXT_SIZE_XS))
                                        .child("✎"),
                                );
                            }
                        }
                        cell
                    });

                    // Leading row-number cell (follows the sort order).
                    let seq = div()
                        .w(px(theme::SEQ_WIDTH))
                        .flex_none()
                        .px(px(theme::PAD))
                        .flex()
                        .items_center()
                        .justify_end()
                        .border_r_1()
                        .border_color(rgb(theme::BORDER))
                        .font_family(theme::FONT_MONO)
                        .text_color(rgb(theme::TEXT_DIM))
                        .text_size(px(theme::TEXT_SIZE_XS))
                        .child(SharedString::from((display_ix + 1).to_string()));

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
                        .child(seq)
                        .children(cells)
                })
                .collect::<Vec<_>>()
        })
        .w(px(total_width))
        .flex_grow()
        .min_h_0()
    }

    /// Footer: row/col summary + CSV/JSON export buttons.
    fn render_footer(&self, result: &QueryResult, arc: Arc<QueryResult>, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = format!("{} rows × {} cols", result.row_count(), result.col_count());

        // One small export button: saves to file via system dialog or copies to clipboard.
        let export_btn = |id: &'static str,
                          label: &'static str,
                          arc: Arc<QueryResult>,
                          use_csv: bool,
                          to_file: bool,
                          cx: &mut Context<Self>| {
            div()
                .id(id)
                .px(px(theme::PAD_SM))
                .py(px(1.))
                .rounded(px(theme::RADIUS_SM))
                .text_size(px(theme::TEXT_SIZE_XS))
                .text_color(rgb(theme::TEXT_DIM))
                .hover(|s| s.bg(rgb(theme::HOVER)).text_color(rgb(theme::TEXT)))
                .on_click(cx.listener(move |_this, _ev, _window, cx| {
                    let data = if use_csv {
                        crate::ui::export::to_csv(&arc)
                    } else {
                        crate::ui::export::to_json(&arc)
                    };
                    if to_file {
                        let ext = if use_csv { "export.csv" } else { "export.json" };
                        let dir = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
                        let rx = cx.prompt_for_new_path(&dir, Some(ext));
                        cx.spawn(async move |_weak, _cx| {
                            if let Ok(Ok(Some(path))) = rx.await {
                                if let Err(e) = std::fs::write(&path, data) {
                                    eprintln!("quill: export failed: {e}");
                                }
                            }
                        })
                        .detach();
                    } else {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(data));
                    }
                }))
                .child(label)
        };

        div()
            .h(px(theme::ROW_HEIGHT))
            .px(px(theme::PAD_LG))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::PAD_SM))
            .bg(rgb(theme::SURFACE))
            .border_t_1()
            .border_color(rgb(theme::BORDER))
            .text_color(rgb(theme::TEXT_DIM))
            .text_size(px(theme::TEXT_SIZE_XS))
            .child(SharedString::from("Result"))
            .child(div().flex_grow())
            // Export to file
            .child(export_btn("exp-csv-file",  "CSV ↓",  arc.clone(), true,  true,  cx))
            .child(export_btn("exp-json-file", "JSON ↓", arc.clone(), false, true,  cx))
            // Copy to clipboard
            .child(export_btn("exp-csv-clip",  "CSV ⎘",  arc.clone(), true,  false, cx))
            .child(export_btn("exp-json-clip", "JSON ⎘", arc.clone(), false, false, cx))
            .child(div().w(px(theme::PAD_LG)))
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
        // Soft-wrap long lines ourselves: gpui won't break a long run that has
        // no whitespace (e.g. minified JSON), so it would overflow horizontally.
        // Inserting newlines at a fixed width guarantees it fits and the
        // overflow_y_scroll shows the rest. Width tracks the measured panel.
        // Wrap budget from the measured panel width (≈7.5px per mono column).
        // Because wrap_lines keeps every line ≤ this budget, the text never
        // stretches the panel, so the measurement stays accurate frame-to-frame
        // (90-col fallback on the very first frame before measurement lands).
        let wrap_cols = if self.detail_width > 50.0 {
            ((self.detail_width / 7.5) as usize).max(20)
        } else {
            90
        };
        let fmt = self.detail_format;
        let formatted = format_value(&raw, fmt);
        let shown = wrap_lines(&formatted, wrap_cols);
        let measure_detail = cx.entity();

        let is_editing_this = self.editing == Some((display_row, c));
        let editable = self.editable;
        let edit_input = self.edit_input.clone();

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
                // Toolbar: column name + format buttons + optional edit controls.
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
                        // Format buttons (hidden while editing so the toolbar stays compact).
                        .when(!is_editing_this, |this| {
                            this.child(self.format_button(DetailFormat::Raw, "Raw", cx))
                                .child(self.format_button(DetailFormat::Json, "JSON", cx))
                                .child(self.format_button(DetailFormat::Base64, "Base64", cx))
                                .child(self.format_button(DetailFormat::Url, "URL", cx))
                                .child(self.format_button(DetailFormat::Timestamp, "Time", cx))
                        })
                        // Edit button (only for editable tables, not while already editing).
                        .when(editable && !is_editing_this, |this| {
                            this.child(
                                div()
                                    .id("detail-edit-btn")
                                    .px(px(theme::PAD_SM))
                                    .py(px(1.))
                                    .rounded(px(theme::RADIUS_SM))
                                    .font_family(theme::FONT_UI)
                                    .text_size(px(theme::TEXT_SIZE_XS))
                                    .bg(rgb(theme::SURFACE))
                                    .text_color(rgb(theme::ACCENT))
                                    .hover(|s| s.bg(rgb(theme::HOVER)))
                                    .on_click(cx.listener(move |this, _ev, window, cx| {
                                        this.start_edit(display_row, c, window, cx);
                                    }))
                                    .child("Edit"),
                            )
                        })
                        // Save / Cancel buttons while editing.
                        .when(is_editing_this, |this| {
                            this.child(
                                div()
                                    .id("detail-save-btn")
                                    .px(px(theme::PAD_SM))
                                    .py(px(1.))
                                    .rounded(px(theme::RADIUS_SM))
                                    .font_family(theme::FONT_UI)
                                    .text_size(px(theme::TEXT_SIZE_XS))
                                    .bg(rgb(theme::ACCENT))
                                    .text_color(rgb(theme::BG_DEEP))
                                    .hover(|s| s.bg(rgb(theme::ACCENT_DIM)))
                                    .on_click(cx.listener(|this, _ev, _w, cx| {
                                        this.commit_edit(cx);
                                    }))
                                    .child("Save"),
                            )
                            .child(
                                div()
                                    .id("detail-cancel-btn")
                                    .px(px(theme::PAD_SM))
                                    .py(px(1.))
                                    .rounded(px(theme::RADIUS_SM))
                                    .font_family(theme::FONT_UI)
                                    .text_size(px(theme::TEXT_SIZE_XS))
                                    .bg(rgb(theme::SURFACE))
                                    .text_color(rgb(theme::TEXT_DIM))
                                    .hover(|s| s.bg(rgb(theme::HOVER)).text_color(rgb(theme::DANGER)))
                                    .on_click(cx.listener(|this, _ev, _w, cx| {
                                        this.cancel_edit(cx);
                                    }))
                                    .child("Cancel"),
                            )
                        }),
                )
                // Value area: show inline edit input or formatted read-only text.
                .child(
                    div()
                        .id("detail-value")
                        .relative()
                        .flex_grow()
                        .min_h_0()
                        .w_full()
                        .overflow_y_scroll()
                        .p(px(theme::PAD_LG))
                        // Measure the value area's width to drive soft-wrapping.
                        .child(
                            canvas(
                                move |bounds, _, cx| {
                                    let w = f32::from(bounds.size.width);
                                    measure_detail.update(cx, |this, _| {
                                        if (this.detail_width - w).abs() > 1.0 {
                                            this.detail_width = w;
                                        }
                                    });
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(px(1.)),
                        )
                        .when(is_editing_this, |this| this.child(edit_input))
                        .when(!is_editing_this, |this| {
                            this.child(detail_value_body(&shown, fmt))
                        }),
                ),
        )
    }
}

/// The value text for the detail panel: a narrow line-number gutter + content.
/// In JSON mode the content is syntax-highlighted; otherwise plain monospace.
/// Renders line-by-line so `\n`s from `wrap_lines` stack vertically.
fn detail_value_body(shown: &str, fmt: DetailFormat) -> gpui::AnyElement {
    let lines: Vec<&str> = shown.split('\n').collect();
    let line_count = lines.len();

    // Gutter: right-aligned line numbers, narrower than the SQL editor gutter.
    let gutter = div()
        .w(px(theme::DETAIL_GUTTER_WIDTH))
        .flex_none()
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(rgb(theme::BORDER))
        .font_family(theme::FONT_MONO)
        .text_size(px(theme::TEXT_SIZE_XS))
        .text_color(rgb(theme::TEXT_DIM))
        .children((1..=line_count).map(|n| {
            div()
                .w_full()
                .pr(px(3.))
                .flex()
                .justify_end()
                .child(SharedString::from(n.to_string()))
        }));

    // Content column.
    let content: gpui::AnyElement = if fmt == DetailFormat::Json {
        let mut col = div()
            .flex_grow()
            .pl(px(theme::PAD_SM))
            .font_family(theme::FONT_MONO)
            .text_size(px(theme::TEXT_SIZE_SM))
            .flex()
            .flex_col();
        for line in &lines {
            let line_toks = tokenize_json(line);
            let mut row = div().flex().flex_row().flex_wrap();
            for (range, kind) in line_toks {
                let seg = &line[range];
                if seg.is_empty() {
                    continue;
                }
                row = row.child(
                    div()
                        .whitespace_nowrap()
                        .text_color(rgb(json_color(kind)))
                        .child(SharedString::from(seg.to_string())),
                );
            }
            col = col.child(row);
        }
        col.into_any_element()
    } else {
        let mut col = div()
            .flex_grow()
            .pl(px(theme::PAD_SM))
            .font_family(theme::FONT_MONO)
            .text_size(px(theme::TEXT_SIZE_SM))
            .text_color(rgb(theme::TEXT))
            .flex()
            .flex_col();
        for line in &lines {
            col = col.child(
                div()
                    .w_full()
                    .child(SharedString::from(line.to_string())),
            );
        }
        col.into_any_element()
    };

    div()
        .w_full()
        .flex()
        .flex_row()
        .child(gutter)
        .child(content)
        .into_any_element()
}

/// Dracula colors for JSON tokens (shared with SQL syntax palette).
fn json_color(kind: crate::ui::detail_format::JsonTok) -> u32 {
    use crate::ui::detail_format::JsonTok::*;
    match kind {
        Key => theme::SYN_KEYWORD,
        Str => theme::SYN_STRING,
        Number => theme::SYN_NUMBER,
        Keyword => theme::SYN_NUMBER,
        Punct => theme::TEXT_DIM,
        Plain => theme::TEXT,
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
                let total_width =
                    theme::SEQ_WIDTH + theme::COL_WIDTH * result.col_count().max(1) as f32;
                let arc_export = arc.clone(); // kept for footer export buttons
                let scroll_x = self.scroll_x;
                let selected = self.selected;
                let editing = self.editing;
                let editable = self.editable;
                let edit_input = self.edit_input.clone();
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
                    .child(self.render_body(arc, total_width, selected, editing, edit_input, editable, order, entity));

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
                    .child(self.render_footer(result, arc_export, cx))
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

/// Soft-wrap each line of `s` to roughly `width_budget` monospace columns,
/// breaking anywhere (so minified JSON / no-whitespace runs still fit).
/// CJK/wide chars count as 2 columns so mixed Chinese/English wraps correctly.
/// Preserves existing newlines; UTF-8 safe (operates on chars).
fn wrap_lines(s: &str, width_budget: usize) -> String {
    let budget = width_budget.max(8);
    let char_w = |c: char| if (c as u32) >= 0x1100 && is_wide(c) { 2 } else { 1 };
    let mut out = String::with_capacity(s.len() + s.len() / 16);
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut w = 0usize;
        for ch in line.chars() {
            let cw = char_w(ch);
            if w + cw > budget {
                out.push('\n');
                w = 0;
            }
            out.push(ch);
            w += cw;
        }
    }
    out
}

/// Rough "is this char double-width" test (CJK, fullwidth, kana, etc.).
fn is_wide(c: char) -> bool {
    let u = c as u32;
    (0x1100..=0x115F).contains(&u)        // Hangul Jamo
        || (0x2E80..=0xA4CF).contains(&u) // CJK radicals … Yi
        || (0xAC00..=0xD7A3).contains(&u) // Hangul syllables
        || (0xF900..=0xFAFF).contains(&u) // CJK compat ideographs
        || (0xFF00..=0xFF60).contains(&u) // Fullwidth forms
        || (0x20000..=0x3FFFD).contains(&u) // CJK ext B+
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
