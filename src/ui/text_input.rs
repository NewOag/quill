//! A hand-rolled single-line text input for Quill.
//!
//! gpui 0.2.2 ships no text-input widget (Zed's real editor lives in a separate
//! crate), so this is adapted from gpui's `examples/input.rs`: it handles
//! keystrokes, cursor, selection, clipboard, and IME (marked text) via
//! [`EntityInputHandler`], and paints itself through a custom [`Element`]
//! (`TextElement`).
//!
//! Quill-specific changes vs. the example:
//! - Catppuccin theme colors instead of the demo's grey/white.
//! - A `key_context` of `"QuillInput"` so key bindings are scoped to the input,
//!   not global (see `main.rs`).
//! - An extra `Submit` action (Enter) that emits [`InputEvent::Submit`] — this
//!   is how the SQL editor runs a query. The example had no Enter handling.
//! - `content` getter/setter so the owning view can read/seed the SQL text.
//!
//! Scope: single line. Multi-line editing (newlines, vertical cursor movement,
//! per-line shaping) is a deliberate follow-up; Enter is bound to "run" here,
//! which is the common database-tool interaction anyway.
//!
//! Defined in full before any view uses it.

use std::ops::Range;

use gpui::{
    actions, div, fill, point, prelude::*, px, relative, rgb, rgba, size, App, Bounds,
    ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::ui::sql_highlight::{tokenize_sql, TokenKind};
use crate::ui::theme;

/// Map a SQL token kind to its display color (raw RGB).
fn syntax_color(kind: TokenKind) -> u32 {
    match kind {
        TokenKind::Keyword => theme::SYN_KEYWORD,
        TokenKind::Str => theme::SYN_STRING,
        TokenKind::Number => theme::SYN_NUMBER,
        TokenKind::Comment => theme::SYN_COMMENT,
        TokenKind::Punct => theme::SYN_PUNCT,
        TokenKind::Plain => theme::SYN_IDENT,
    }
}

// Actions scoped to the input. Bound to keys in `main.rs` with the
// "QuillInput" context so they don't fire globally.
actions!(
    quill_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        Submit,
        Newline,
        MoveUp,
        MoveDown,
    ]
);

/// Emitted by the input. The owning view (the SQL editor) subscribes.
#[derive(Debug, Clone)]
pub enum InputEvent {
    /// Enter pressed — run the current content.
    Submit,
}

/// An editable text field. Supports multiple lines separated by `\n`.
/// In single-line form fields `highlight_sql` is false and newlines are
/// prevented by the action bindings; in the SQL editor it is true and Enter
/// inserts `\n`.
pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    /// Cached per-line shaped text from the last paint, used for cursor
    /// placement and mouse hit-testing. Index = logical line number.
    last_lines: Vec<ShapedLine>,
    /// Byte offset of the start of each logical line (same indexing as
    /// `last_lines`). Computed from `content` at paint time.
    last_line_starts: Vec<usize>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    /// When true, content is tokenized as SQL and colored.
    highlight_sql: bool,
}

impl EventEmitter<InputEvent> for TextInput {}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, placeholder: impl Into<SharedString>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_lines: Vec::new(),
            last_line_starts: vec![0],
            last_bounds: None,
            is_selecting: false,
            highlight_sql: false,
        }
    }

    /// Enable SQL syntax highlighting on this input (builder style).
    pub fn with_sql_highlight(mut self) -> Self {
        self.highlight_sql = true;
        self
    }

    /// Current text.
    pub fn content(&self) -> SharedString {
        self.content.clone()
    }

    /// Replace the text (e.g. seed the editor with a generated SELECT). Resets
    /// the cursor to the end.
    pub fn set_content(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.marked_range = None;
        cx.notify();
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Submit);
    }

    // --- cursor / selection movement (verbatim logic from the example) -----

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        // Line-relative: go to the start of the current line.
        let (row, _) = self.offset_to_row_col(self.cursor_offset());
        self.move_to(self.line_starts()[row], cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        // Line-relative: go to the end of the current line (before '\n').
        let (row, _) = self.offset_to_row_col(self.cursor_offset());
        let starts = self.line_starts();
        let line_end = if row + 1 < starts.len() {
            starts[row + 1].saturating_sub(1) // before '\n'
        } else {
            self.content.len()
        };
        self.move_to(line_end, cx);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, cx);
    }

    fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, cx);
    }

    fn move_vertical(&mut self, delta: isize, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let (row, col) = self.offset_to_row_col(cursor);
        let line_count = self.line_count();
        let new_row = (row as isize + delta).clamp(0, line_count as isize - 1) as usize;
        if new_row == row {
            return;
        }
        // Try to keep the same column; clamp to the target line's length.
        let new_offset = self.row_col_to_offset(new_row, col);
        self.move_to(new_offset, cx);
    }

    // --- multiline helpers -------------------------------------------------

    /// Byte offset of the start of each logical line.
    pub fn line_starts(&self) -> Vec<usize> {
        let mut v = vec![0usize];
        for (i, b) in self.content.bytes().enumerate() {
            if b == b'\n' {
                v.push(i + 1);
            }
        }
        v
    }

    pub fn line_count(&self) -> usize {
        self.content.matches('\n').count() + 1
    }

    /// The text of a logical line (without the trailing `\n`).
    pub fn line_text(&self, row: usize) -> &str {
        let starts = self.line_starts();
        let s = starts.get(row).copied().unwrap_or(self.content.len());
        let e = starts
            .get(row + 1)
            .copied()
            .map(|n| n.saturating_sub(1)) // strip '\n'
            .unwrap_or(self.content.len());
        &self.content[s..e]
    }

    pub fn offset_to_row_col(&self, offset: usize) -> (usize, usize) {
        let starts = self.line_starts();
        let row = starts.partition_point(|&s| s <= offset).saturating_sub(1);
        let col = offset - starts[row];
        (row, col)
    }

    pub fn row_col_to_offset(&self, row: usize, col: usize) -> usize {
        let starts = self.line_starts();
        let line_len = self.line_text(row).len();
        starts.get(row).copied().unwrap_or(self.content.len()) + col.min(line_len)
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // Single-line: collapse newlines so a pasted multi-line query stays
            // on one line for now.
            // Preserve newlines: multi-line paste works for the SQL editor.
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() || self.last_lines.is_empty() {
            return 0;
        }
        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        // Which visual row did the click land on?
        // We need line_height — we stored line starts but not px heights. Use a
        // heuristic: divide remaining space evenly. For exact height we'd need a
        // stored line_height; instead, use the count of lines.
        let rel_y = f32::from(position.y - bounds.top());
        let total_h = f32::from(bounds.bottom() - bounds.top());
        let n = self.last_lines.len().max(1);
        let line_h = total_h / n as f32;
        let row = ((rel_y / line_h) as usize).min(n - 1);
        let col = self.last_lines[row].closest_index_for_x(position.x - bounds.left());
        self.row_col_to_offset(row, col)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    // --- UTF-8 <-> UTF-16 conversions (platform IME speaks UTF-16) ---------

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let starts = &self.last_line_starts;
        if starts.is_empty() { return None; }
        // Use the start of the range to find the line; return a single-line rect.
        let row = starts.partition_point(|&s| s <= range.start).saturating_sub(1);
        let line = self.last_lines.get(row)?;
        let col_start = range.start - starts[row];
        let col_end = (range.end - starts[row]).min(line.len());
        let line_height = if self.last_lines.len() > 1 {
            f32::from(bounds.size.height) / self.last_lines.len() as f32
        } else {
            f32::from(bounds.size.height)
        };
        let y = bounds.top() + gpui::px(line_height * row as f32);
        Some(Bounds::from_corners(
            point(bounds.left() + line.x_for_index(col_start), y),
            point(bounds.left() + line.x_for_index(col_end), y + gpui::px(line_height)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        pt: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let _ = bounds.localize(&pt)?;
        let n = self.last_lines.len().max(1);
        let total_h = f32::from(bounds.size.height);
        let line_h = total_h / n as f32;
        let rel_y = f32::from(pt.y - bounds.top());
        let row = ((rel_y / line_h) as usize).min(n - 1);
        let line = self.last_lines.get(row)?;
        let col = line.index_for_x(pt.x - bounds.left()).unwrap_or(0);
        let utf8_idx = self.row_col_to_offset(row, col);
        Some(self.offset_to_utf16(utf8_idx))
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .key_context("QuillInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::move_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .size_full()
            .child(TextElement { input: cx.entity() })
    }
}

/// Extract the sub-runs covering byte range `[ls, le)` of the full text from
/// a global run list. Needed to shape each line independently while keeping
/// syntax-highlighting and IME runs aligned to correct byte positions.
/// Precondition: `runs` together cover `0..full_text.len()` with no gaps.
fn runs_for_slice(runs: &[TextRun], ls: usize, le: usize, template: &TextRun) -> Vec<TextRun> {
    if ls >= le {
        // Empty line (e.g. after a trailing '\n'): return a zero-len run so
        // shape_line gets a non-empty slice and doesn't panic.
        return vec![TextRun { len: 0, ..template.clone() }];
    }
    let mut result = Vec::new();
    let mut pos = 0;
    for r in runs {
        let run_end = pos + r.len;
        if run_end <= ls { pos = run_end; continue; }
        if pos >= le { break; }
        let slice_start = pos.max(ls);
        let slice_end = run_end.min(le);
        if slice_start < slice_end {
            result.push(TextRun { len: slice_end - slice_start, ..r.clone() });
        }
        pos = run_end;
    }
    if result.is_empty() || result.iter().map(|r| r.len).sum::<usize>() == 0 {
        result = vec![TextRun { len: le - ls, ..template.clone() }];
    }
    result
}

/// The custom element that shapes and paints the input's text, cursor, and
/// selection, and registers the IME input handler during paint.
struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    /// One shaped line per logical `\n`-separated line.
    lines: Vec<ShapedLine>,
    line_starts: Vec<usize>,
    cursor: Option<PaintQuad>,
    /// Selection quads, one per visual line the selection spans.
    selections: Vec<PaintQuad>,
    line_height: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let line_count = self.input.read(cx).line_count().max(1);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = (window.line_height() * line_count as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor_offset = input.cursor_offset();
        let highlight_sql = input.highlight_sql;
        let marked_range = input.marked_range.clone();
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();

        // If empty, show the placeholder as a single line.
        let (show_placeholder, display) = if content.is_empty() {
            (true, input.placeholder.clone())
        } else {
            (false, content.clone())
        };

        // Split by \n into logical lines. shape_line panics on \n so we must strip them.
        let line_starts: Vec<usize> = {
            let mut v = vec![0usize];
            for (i, b) in display.bytes().enumerate() {
                if b == b'\n' { v.push(i + 1); }
            }
            v
        };

        // Build per-line runs, applying SQL highlighting and IME underline.
        let template = TextRun {
            len: 0,
            font: style.font(),
            color: if show_placeholder { rgb(theme::TEXT_DIM).into() } else { style.color },
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        // Global runs over display text (for sql highlight / IME).
        let global_runs: Vec<TextRun> = if highlight_sql && !show_placeholder {
            tokenize_sql(&display)
                .into_iter()
                .map(|(range, kind)| TextRun {
                    len: range.len(),
                    color: rgb(syntax_color(kind)).into(),
                    ..template.clone()
                })
                .collect()
        } else {
            vec![TextRun { len: display.len(), ..template.clone() }]
        };

        // Layer IME underline.
        let underlined_runs: Vec<TextRun> = if let Some(ref marked) = marked_range {
            let ul = UnderlineStyle { color: Some(template.color), thickness: px(1.0), wavy: false };
            let mut out = Vec::new();
            let mut pos = 0;
            for r in global_runs {
                let start = pos; let end = pos + r.len; pos = end;
                let mi = marked.start.clamp(start, end);
                let mj = marked.end.clamp(start, end);
                if start < mi { out.push(TextRun { len: mi - start, ..r.clone() }); }
                if mi < mj   { out.push(TextRun { len: mj - mi, underline: Some(ul), ..r.clone() }); }
                if mj < end  { out.push(TextRun { len: end - mj, ..r.clone() }); }
            }
            out.into_iter().filter(|r| r.len > 0).collect()
        } else {
            global_runs
        };

        // Shape one ShapedLine per logical line.
        let n_lines = line_starts.len();
        let mut lines: Vec<ShapedLine> = Vec::with_capacity(n_lines);
        for row in 0..n_lines {
            let ls = line_starts[row];
            let le = if row + 1 < n_lines {
                line_starts[row + 1].saturating_sub(1) // exclude '\n'
            } else {
                display.len()
            };
            let line_text = SharedString::from(display[ls..le].to_string());
            // Extract sub-runs that overlap this line.
            let line_runs = runs_for_slice(&underlined_runs, ls, le, &template);
            let shaped = window.text_system().shape_line(
                line_text, font_size, &line_runs, None,
            );
            lines.push(shaped);
        }

        // Cursor position.
        let cursor_quad = {
            let (row, col) = {
                let r = line_starts.partition_point(|&s| s <= cursor_offset).saturating_sub(1);
                (r, cursor_offset - line_starts[r])
            };
            let x = lines.get(row).map(|l| l.x_for_index(col)).unwrap_or(px(0.));
            let y = bounds.top() + line_height * row as f32;
            fill(
                Bounds::new(point(bounds.left() + x, y), size(px(2.), line_height)),
                rgb(theme::ACCENT),
            )
        };

        // Selection quads — one per line the selection spans.
        let mut selections = Vec::new();
        if !selected_range.is_empty() {
            let sel_color = rgba(0x89b4fa40);
            let (s_row, s_col) = {
                let r = line_starts.partition_point(|&s| s <= selected_range.start).saturating_sub(1);
                (r, selected_range.start - line_starts[r])
            };
            let (e_row, e_col) = {
                let r = line_starts.partition_point(|&s| s <= selected_range.end).saturating_sub(1);
                (r, selected_range.end - line_starts[r])
            };
            for row in s_row..=e_row {
                let line_w = lines.get(row).map(|l| l.x_for_index(l.len())).unwrap_or(px(0.));
                let x0 = if row == s_row {
                    lines.get(row).map(|l| l.x_for_index(s_col)).unwrap_or(px(0.))
                } else {
                    px(0.)
                };
                let x1 = if row == e_row {
                    lines.get(row).map(|l| l.x_for_index(e_col)).unwrap_or(line_w)
                } else {
                    line_w + px(6.) // extend slightly past end of line to show newline included
                };
                let y = bounds.top() + line_height * row as f32;
                selections.push(fill(
                    Bounds::from_corners(
                        point(bounds.left() + x0, y),
                        point(bounds.left() + x1, y + line_height),
                    ),
                    sel_color,
                ));
            }
        }

        let cursor = selected_range.is_empty().then_some(cursor_quad);
        PrepaintState { lines, line_starts, cursor, selections, line_height }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );

        // Selection quads (paint before text so text is on top).
        for sel in prepaint.selections.drain(..) {
            window.paint_quad(sel);
        }

        // Paint each logical line.
        let line_height = prepaint.line_height;
        for (row, line) in prepaint.lines.iter().enumerate() {
            let origin = point(bounds.left(), bounds.top() + line_height * row as f32);
            line.paint(origin, line_height, window, cx).ok();
        }

        // Cursor (only when focused).
        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }

        // Write back cached per-line layout for mouse hit-testing.
        let lines = prepaint.lines.clone();
        let line_starts = prepaint.line_starts.clone();
        self.input.update(cx, |input, _| {
            input.last_lines = lines;
            input.last_line_starts = line_starts;
            input.last_bounds = Some(bounds);
        });
    }
}
