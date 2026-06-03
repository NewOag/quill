//! SQL editor pane: a real editable text input plus a Run button.
//!
//! Holds a [`TextInput`] entity (the hand-rolled control in
//! [`crate::ui::text_input`]). Running a query happens two ways, both emitting
//! [`EditorEvent::Run`] with the current text:
//! - pressing Enter inside the input (the input emits
//!   [`InputEvent::Submit`], which we subscribe to), or
//! - clicking the Run button.
//!
//! The Workspace subscribes to [`EditorEvent`] and executes the SQL. The editor
//! never touches the database itself.
//!
//! Defined in full before the Workspace uses it.

use gpui::{
    div, prelude::*, px, rgb, Context, Entity, EventEmitter, Focusable, SharedString, Window,
};

use crate::ui::text_input::{InputEvent, TextInput};
use crate::ui::theme;

/// Emitted by the editor. The Workspace subscribes and runs the SQL.
#[derive(Debug, Clone)]
pub enum EditorEvent {
    /// Execute this SQL.
    Run(String),
}

/// The SQL editor view.
pub struct QueryEditor {
    input: Entity<TextInput>,
}

impl EventEmitter<EditorEvent> for QueryEditor {}

impl QueryEditor {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextInput::new(cx, "Type SQL… (Enter=newline, Cmd+Enter=run)").with_sql_highlight()
        });

        // When the input emits Submit (Enter), run its current content.
        cx.subscribe(&input, |this, input, event, cx| match event {
            InputEvent::Submit => {
                let sql = input.read(cx).content().to_string();
                if !sql.trim().is_empty() {
                    cx.emit(EditorEvent::Run(sql));
                }
                let _ = this;
            }
        })
        .detach();

        Self { input }
    }

    /// Seed the editor with SQL (e.g. the generated SELECT from a table click).
    pub fn set_sql(&mut self, sql: impl Into<String>, cx: &mut Context<Self>) {
        let sql = sql.into();
        self.input.update(cx, |input, cx| {
            input.set_content(sql, cx);
        });
    }

    /// Focus the input (called by the Workspace on startup so the user can type
    /// immediately).
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            window.focus(&input.focus_handle(cx));
        });
    }

    fn run_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let input = self.input.clone();
        div()
            .id("run-btn")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::PAD_XS))
            .px(px(theme::PAD_LG))
            .py(px(theme::PAD_XS))
            .rounded(px(theme::RADIUS))
            .bg(rgb(theme::ACCENT))
            .text_color(rgb(theme::BG_DEEP))
            .text_size(px(theme::TEXT_SIZE_SM))
            .hover(|s| s.opacity(0.85))
            .active(|s| s.opacity(0.7))
            .on_click(cx.listener(move |_this, _ev, _window, cx| {
                let sql = input.read(cx).content().to_string();
                if !sql.trim().is_empty() {
                    cx.emit(EditorEvent::Run(sql));
                }
            }))
            .child(theme::ICON_RUN)
            .child("Run")
    }

    /// Left line-number gutter. Line count is derived from the input content
    /// (`\n` count + 1), so it's correct for single- and multi-line SQL alike.
    /// Top padding + text size match the input so numbers align with text rows.
    fn line_gutter(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self.input.read(cx).content();
        let line_count = content.matches('\n').count() + 1;
        div()
            .w(px(theme::GUTTER_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .py(px(theme::PAD_LG))
            .bg(rgb(theme::BG_DEEP))
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .font_family(theme::FONT_MONO)
            .text_size(px(theme::TEXT_SIZE))
            .text_color(rgb(theme::TEXT_DIM))
            .children((1..=line_count).map(|n| {
                div()
                    .w_full()
                    .pr(px(theme::PAD_SM))
                    .flex()
                    .justify_end()
                    .child(SharedString::from(n.to_string()))
            }))
    }
}

impl Render for QueryEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(theme::EDITOR_HEIGHT))
            .w_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::BG_PANEL))
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            // Toolbar row.
            .child(
                div()
                    .h(px(theme::ROW_HEIGHT))
                    .px(px(theme::PAD_LG))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .bg(rgb(theme::SURFACE))
                    .border_b_1()
                    .border_color(rgb(theme::BORDER))
                    .child(
                        div()
                            .text_color(rgb(theme::TEXT_DIM))
                            .text_size(px(theme::TEXT_SIZE_XS))
                            .child("SQL"),
                    )
                    .child(self.run_button(cx)),
            )
            // Editable input area — a line-number gutter + the monospace input.
            // The content area scrolls vertically when the SQL grows taller
            // than the pane; the input element's height auto-grows with lines.
            .child(
                div()
                    .id("editor-scroll")
                    .flex_grow()
                    .overflow_y_scroll()
                    .flex()
                    .flex_row()
                    .child(self.line_gutter(cx))
                    .child(
                        div()
                            .flex_grow()
                            .py(px(theme::PAD_LG))
                            .pr(px(theme::PAD_LG))
                            .font_family(theme::FONT_MONO)
                            .text_color(rgb(theme::TEXT))
                            .text_size(px(theme::TEXT_SIZE))
                            .child(self.input.clone()),
                    ),
            )
    }
}
