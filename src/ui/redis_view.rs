//! Redis-specific main view: command-line input + key detail panel.
//!
//! Layout (top → bottom, full height):
//! ```text
//! ┌──────────────────────────────────────────────────────────────┐
//! │ CMD>  [input]                           [Run]  [History ▾]  │  (cmd bar)
//! ├──────────────────────────────────────────────────────────────┤
//! │ (collapsible command history)                                │
//! ├──────────────────────────────────────────────────────────────┤
//! │ key name · type · TTL                                        │  (info bar)
//! ├──────────────────────────────────────────────────────────────┤
//! │ value area (string: big text / hash: 2-col / list: rows)     │  (flex_grow)
//! └──────────────────────────────────────────────────────────────┘
//! ```

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, rgb, uniform_list, Context, Entity, SharedString, Task, Window,
};

use crate::app::db::Db;
use crate::datasource::redis::{RedisKeyDetail, RedisValue};
use crate::ui::detail_format::{format_value, DetailFormat};
use crate::ui::text_input::{InputEvent, TextInput};
use crate::ui::theme;

pub struct RedisView {
    db: Option<Db>,
    /// Redis command input (no SQL highlighting — plain text).
    cmd_input: Entity<TextInput>,
    /// History as `(command, response)` pairs, newest first.
    cmd_history: Vec<(String, String)>,
    show_history: bool,
    /// The key currently selected in the sidebar.
    current_key: Option<Arc<RedisKeyDetail>>,
    /// Which format transform to apply for string values.
    detail_format: DetailFormat,
    _cmd_task: Option<Task<()>>,
    _key_task: Option<Task<()>>,
}

impl RedisView {
    pub fn new(db: Option<Db>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cmd_input = cx.new(|cx| TextInput::new(cx, "Redis command (e.g. DBSIZE)"));
        // Submit from the command input fires a command.
        cx.subscribe(&cmd_input, |this: &mut Self, _input, event: &InputEvent, cx| {
            match event {
                InputEvent::Submit => this.run_cmd(cx),
            }
        })
        .detach();

        Self {
            db,
            cmd_input,
            cmd_history: Vec::new(),
            show_history: false,
            current_key: None,
            detail_format: DetailFormat::Raw,
            _cmd_task: None,
            _key_task: None,
        }
    }

    /// Load the detail for a key (called when user clicks a key in the sidebar).
    pub fn load_key(&mut self, key: String, cx: &mut Context<Self>) {
        let Some(db) = self.db.clone() else { return };
        let handle = db.handle();
        self._key_task = Some(cx.spawn(async move |weak, cx| {
            let (tx, rx) = futures::channel::oneshot::channel();
            handle.spawn(async move {
                let _ = tx.send(db.redis_get(&key).await);
            });
            if let Ok(Ok(detail)) = rx.await {
                let _ = weak.update(cx, |this, cx| {
                    this.current_key = Some(Arc::new(detail));
                    this.detail_format = DetailFormat::Raw;
                    cx.notify();
                });
            }
        }));
    }

    /// Execute the text in the command input.
    fn run_cmd(&mut self, cx: &mut Context<Self>) {
        let cmd = self.cmd_input.read(cx).content().to_string();
        if cmd.trim().is_empty() {
            return;
        }
        let Some(db) = self.db.clone() else { return };
        let handle = db.handle();
        self._cmd_task = Some(cx.spawn(async move |weak, cx| {
            let (tx, rx) = futures::channel::oneshot::channel();
            let cmd_for_log = cmd.clone();
            handle.spawn(async move {
                let _ = tx.send(db.redis_cmd(&cmd).await);
            });
            let output = match rx.await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => format!("(error) {e:#}"),
                Err(_) => "(task dropped)".into(),
            };
            let _ = weak.update(cx, |this, cx| {
                this.cmd_history.insert(0, (cmd_for_log, output));
                this.show_history = true;
                cx.notify();
            });
        }));
    }

    // --- rendering sub-helpers ---

    fn render_cmd_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let history_label = if self.show_history { "History ▴" } else { "History ▾" };
        div()
            .flex_none()
            .h(px(theme::ROW_HEIGHT * 1.4))
            .px(px(theme::PAD_LG))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::PAD_SM))
            .bg(rgb(theme::SURFACE))
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            // "CMD>" label
            .child(
                div()
                    .text_color(rgb(theme::ACCENT))
                    .text_size(px(theme::TEXT_SIZE_XS))
                    .font_family(theme::FONT_MONO)
                    .flex_none()
                    .child("CMD>"),
            )
            // Command input
            .child(
                div()
                    .flex_grow()
                    .font_family(theme::FONT_MONO)
                    .text_size(px(theme::TEXT_SIZE))
                    .child(self.cmd_input.clone()),
            )
            // Run button
            .child(
                div()
                    .id("redis-run")
                    .px(px(theme::PAD_LG))
                    .py(px(theme::PAD_XS))
                    .rounded(px(theme::RADIUS))
                    .bg(rgb(theme::ACCENT))
                    .text_color(rgb(theme::BG_DEEP))
                    .text_size(px(theme::TEXT_SIZE_SM))
                    .hover(|s| s.opacity(0.85))
                    .on_click(cx.listener(|this, _ev, _w, cx| this.run_cmd(cx)))
                    .child(SharedString::from(format!("{} Run", theme::ICON_RUN))),
            )
            // History toggle
            .child(
                div()
                    .id("redis-hist-toggle")
                    .px(px(theme::PAD_SM))
                    .py(px(theme::PAD_XS))
                    .rounded(px(theme::RADIUS))
                    .bg(rgb(theme::BG_PANEL))
                    .text_color(rgb(theme::TEXT_DIM))
                    .text_size(px(theme::TEXT_SIZE_SM))
                    .hover(|s| s.bg(rgb(theme::HOVER)).text_color(rgb(theme::TEXT)))
                    .on_click(cx.listener(|this, _ev, _w, cx| {
                        this.show_history = !this.show_history;
                        cx.notify();
                    }))
                    .child(history_label),
            )
    }

    fn render_history(&self) -> impl IntoElement {
        let items = self.cmd_history.clone();
        div()
            .id("redis-history")
            .flex_none()
            .h(px(theme::ROW_HEIGHT * 5.))
            .overflow_y_scroll()
            .bg(rgb(theme::BG_DEEP))
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            .px(px(theme::PAD_LG))
            .py(px(theme::PAD_SM))
            .font_family(theme::FONT_MONO)
            .text_size(px(theme::TEXT_SIZE_SM))
            .children(items.into_iter().enumerate().map(|(i, (cmd, out))| {
                div()
                    .flex()
                    .flex_col()
                    .mb(px(theme::PAD_SM))
                    // The latest entry is bold/accented.
                    .child(
                        div()
                            .text_color(rgb(theme::ACCENT))
                            .child(SharedString::from(format!("▸ {cmd}"))),
                    )
                    .child(
                        div()
                            .text_color(rgb(if i == 0 { theme::TEXT } else { theme::TEXT_DIM }))
                            .child(SharedString::from(out)),
                    )
            }))
    }

    fn render_info_bar(&self) -> impl IntoElement {
        let (key_name, type_label, ttl_label) = match &self.current_key {
            None => ("select a key".to_string(), String::new(), String::new()),
            Some(d) => (d.key.clone(), d.type_name.clone(), d.ttl_label()),
        };
        div()
            .flex_none()
            .h(px(theme::ROW_HEIGHT))
            .px(px(theme::PAD_LG))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::PAD_LG))
            .bg(rgb(theme::SURFACE))
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            .child(
                div()
                    .flex_grow()
                    .min_w_0()
                    .truncate()
                    .font_family(theme::FONT_MONO)
                    .text_color(rgb(theme::TEXT))
                    .text_size(px(theme::TEXT_SIZE_SM))
                    .child(SharedString::from(key_name)),
            )
            .child(type_badge(&type_label))
            .children((!ttl_label.is_empty()).then(||
                div()
                    .text_color(rgb(theme::TEXT_DIM))
                    .text_size(px(theme::TEXT_SIZE_XS))
                    .child(SharedString::from(format!("TTL: {ttl_label}"))),
            ))
    }

    fn render_value_area(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(detail) = &self.current_key else {
            return div()
                .size_full()
                .flex()
                .justify_center()
                .items_center()
                .text_color(rgb(theme::TEXT_DIM))
                .text_size(px(theme::TEXT_SIZE))
                .child("Click a key in the sidebar to inspect its value")
                .into_any_element();
        };

        match &detail.value {
            RedisValue::Str(s) => self.render_string_value(s.clone(), cx).into_any_element(),
            RedisValue::List(v) => render_indexed_list(v.clone()).into_any_element(),
            RedisValue::Set(v) => render_set_list(v.clone()).into_any_element(),
            RedisValue::Hash(v) => render_hash_table(v.clone()).into_any_element(),
            RedisValue::ZSet(v) => render_zset_table(v.clone()).into_any_element(),
            RedisValue::Unknown(msg) => div()
                .size_full()
                .flex()
                .justify_center()
                .items_center()
                .text_color(rgb(theme::TEXT_DIM))
                .child(SharedString::from(msg.clone()))
                .into_any_element(),
        }
    }

    fn render_string_value(&self, raw: String, cx: &mut Context<Self>) -> impl IntoElement {
        let fmt = self.detail_format;
        let shown = format_value(&raw, fmt);
        div()
            .size_full()
            .flex()
            .flex_col()
            // Format toolbar
            .child(
                div()
                    .flex_none()
                    .h(px(theme::ROW_HEIGHT))
                    .px(px(theme::PAD_LG))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(theme::PAD_SM))
                    .bg(rgb(theme::SURFACE))
                    .border_b_1()
                    .border_color(rgb(theme::BORDER))
                    .children(
                        [
                            (DetailFormat::Raw, "Raw"),
                            (DetailFormat::Json, "JSON"),
                            (DetailFormat::Base64, "Base64"),
                            (DetailFormat::Url, "URL"),
                            (DetailFormat::Timestamp, "Time"),
                        ]
                        .map(|(f, label)| {
                            let active = fmt == f;
                            div()
                                .id(SharedString::from(format!("rfmt-{label}")))
                                .px(px(theme::PAD_SM))
                                .py(px(1.))
                                .rounded(px(theme::RADIUS_SM))
                                .bg(rgb(if active { theme::ACCENT } else { theme::SURFACE }))
                                .text_color(rgb(if active {
                                    theme::BG_DEEP
                                } else {
                                    theme::TEXT_DIM
                                }))
                                .text_size(px(theme::TEXT_SIZE_XS))
                                .hover(|s| s.text_color(rgb(theme::TEXT)))
                                .on_click(cx.listener(move |this, _ev, _w, cx| {
                                    this.detail_format = f;
                                    cx.notify();
                                }))
                                .child(label)
                        }),
                    ),
            )
            // Value text
            .child(
                div()
                    .id("redis-str-val")
                    .flex_grow()
                    .overflow_y_scroll()
                    .p(px(theme::PAD_LG))
                    .font_family(theme::FONT_MONO)
                    .text_size(px(theme::TEXT_SIZE_SM))
                    .text_color(rgb(theme::TEXT))
                    .child(SharedString::from(shown)),
            )
    }
}

// --- free render helpers for collection types ---

fn render_indexed_list(items: Vec<String>) -> impl IntoElement {
    let n = items.len();
    let count_label = format!("{n} element{}", if n == 1 { "" } else { "s" });
    let items = Arc::new(items);
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(list_header(&count_label))
        .child(
            uniform_list("redis-list", n, move |range, _w, _cx| {
                let items = items.clone();
                range
                    .map(|i| {
                        let badge = format!("{i}");
                        let val = items.get(i).cloned().unwrap_or_default();
                        list_row(i, badge, theme::TEXT_DIM, val, theme::TEXT)
                    })
                    .collect()
            })
            .flex_grow()
            .min_h_0(),
        )
}

fn render_set_list(items: Vec<String>) -> impl IntoElement {
    let n = items.len();
    let count_label = format!("{n} member{}", if n == 1 { "" } else { "s" });
    let items = Arc::new(items);
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(list_header(&count_label))
        .child(
            uniform_list("redis-set", n, move |range, _w, _cx| {
                let items = items.clone();
                range
                    .map(|i| {
                        let val = items.get(i).cloned().unwrap_or_default();
                        list_row(i, "•".into(), theme::ACCENT_DIM, val, theme::TEXT)
                    })
                    .collect()
            })
            .flex_grow()
            .min_h_0(),
        )
}

fn render_hash_table(pairs: Vec<(String, String)>) -> impl IntoElement {
    let n = pairs.len();
    let count_label = format!("{n} field{}", if n == 1 { "" } else { "s" });
    let pairs = Arc::new(pairs);
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(two_col_header("field", "value", &count_label))
        .child(
            uniform_list("redis-hash", n, move |range, _w, _cx| {
                let pairs = pairs.clone();
                range
                    .map(|i| {
                        let (f, v) = pairs.get(i)
                            .map(|p| (p.0.clone(), p.1.clone()))
                            .unwrap_or_default();
                        two_col_row(i, f, theme::SYN_KEYWORD, v, theme::TEXT)
                    })
                    .collect()
            })
            .flex_grow()
            .min_h_0(),
        )
}

fn render_zset_table(pairs: Vec<(String, f64)>) -> impl IntoElement {
    let n = pairs.len();
    let count_label = format!("{n} member{}", if n == 1 { "" } else { "s" });
    let pairs = Arc::new(pairs);
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(two_col_header("score", "member", &count_label))
        .child(
            uniform_list("redis-zset", n, move |range, _w, _cx| {
                let pairs = pairs.clone();
                range
                    .map(|i| {
                        let (m, s) = pairs.get(i)
                            .map(|p| (p.0.clone(), p.1))
                            .unwrap_or_default();
                        two_col_row(i, format!("{s:.4}"), theme::SYN_NUMBER, m, theme::TEXT)
                    })
                    .collect()
            })
            .flex_grow()
            .min_h_0(),
        )
}

// --- tiny widget builders ---

fn type_badge(type_name: &str) -> impl IntoElement {
    let (label, color) = match type_name {
        "string" => ("STRING", theme::SYN_STRING),
        "list"   => ("LIST",   theme::SYN_NUMBER),
        "hash"   => ("HASH",   theme::SYN_KEYWORD),
        "set"    => ("SET",    theme::ACCENT),
        "zset"   => ("ZSET",   theme::SYN_COMMENT),
        "stream" => ("STREAM", theme::TEXT_DIM),
        _        => ("?",      theme::TEXT_DIM),
    };
    div()
        .px(px(theme::PAD_SM))
        .py(px(1.))
        .rounded(px(theme::RADIUS_SM))
        .bg(rgb(theme::BG_DEEP))
        .text_color(rgb(color))
        .text_size(px(theme::TEXT_SIZE_XS))
        .font_family(theme::FONT_MONO)
        .child(SharedString::from(label.to_string()))
}

fn list_header(count: &str) -> impl IntoElement {
    div()
        .flex_none()
        .h(px(theme::ROW_HEIGHT))
        .px(px(theme::PAD_LG))
        .flex()
        .items_center()
        .justify_between()
        .bg(rgb(theme::SURFACE))
        .border_b_1()
        .border_color(rgb(theme::BORDER))
        .text_size(px(theme::TEXT_SIZE_XS))
        .text_color(rgb(theme::TEXT_DIM))
        .child(SharedString::from(count.to_string()))
}

fn two_col_header(col1: &str, col2: &str, count: &str) -> impl IntoElement {
    div()
        .flex_none()
        .h(px(theme::ROW_HEIGHT))
        .px(px(theme::PAD_LG))
        .flex()
        .flex_row()
        .items_center()
        .bg(rgb(theme::SURFACE))
        .border_b_1()
        .border_color(rgb(theme::BORDER))
        .text_size(px(theme::TEXT_SIZE_SM))
        .font_family(theme::FONT_UI)
        .child(div().w(px(theme::COL_WIDTH)).text_color(rgb(theme::TEXT)).child(SharedString::from(col1.to_string())))
        .child(div().flex_grow().text_color(rgb(theme::TEXT)).child(SharedString::from(col2.to_string())))
        .child(div().text_color(rgb(theme::TEXT_DIM)).text_size(px(theme::TEXT_SIZE_XS)).child(SharedString::from(count.to_string())))
}

fn list_row(ix: usize, badge: String, badge_color: u32, value: String, val_color: u32) -> impl IntoElement {
    let bg = if ix % 2 == 0 { theme::BG_PANEL } else { theme::ROW_ALT };
    div()
        .id(ix)
        .flex()
        .flex_row()
        .h(px(theme::ROW_HEIGHT))
        .px(px(theme::PAD_LG))
        .bg(rgb(bg))
        .border_b_1()
        .border_color(rgb(theme::BORDER))
        .hover(|s| s.bg(rgb(theme::HOVER)))
        .font_family(theme::FONT_MONO)
        .text_size(px(theme::TEXT_SIZE_SM))
        .child(div().w(px(theme::SEQ_WIDTH)).text_color(rgb(badge_color)).flex_none().child(SharedString::from(badge)))
        .child(div().flex_grow().overflow_hidden().truncate().text_color(rgb(val_color)).child(SharedString::from(value)))
}

fn two_col_row(ix: usize, col1: String, c1_color: u32, col2: String, c2_color: u32) -> impl IntoElement {
    let bg = if ix % 2 == 0 { theme::BG_PANEL } else { theme::ROW_ALT };
    div()
        .id(ix)
        .flex()
        .flex_row()
        .h(px(theme::ROW_HEIGHT))
        .px(px(theme::PAD_LG))
        .bg(rgb(bg))
        .border_b_1()
        .border_color(rgb(theme::BORDER))
        .hover(|s| s.bg(rgb(theme::HOVER)))
        .font_family(theme::FONT_MONO)
        .text_size(px(theme::TEXT_SIZE_SM))
        .child(div().w(px(theme::COL_WIDTH)).flex_none().overflow_hidden().truncate().text_color(rgb(c1_color)).child(SharedString::from(col1)))
        .child(div().flex_grow().overflow_hidden().truncate().text_color(rgb(c2_color)).child(SharedString::from(col2)))
}

impl Render for RedisView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::BG_PANEL))
            // 1. Command bar (always visible)
            .child(self.render_cmd_bar(cx))
            // 2. Command history (collapsible)
            .children(self.show_history.then(|| self.render_history()))
            // 3. Key info bar
            .child(self.render_info_bar())
            // 4. Value area (flex_grow)
            .child(div().flex_grow().min_h_0().child(self.render_value_area(cx)))
    }
}
