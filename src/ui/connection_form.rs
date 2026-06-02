//! The new-connection form, shown as a modal dialog.
//!
//! Six fields, each a reused [`TextInput`] (`crate::ui::text_input`): name,
//! host, port, user, password, database. A Save button validates and emits
//! [`FormEvent::Save`] with a fresh [`ConnectionConfig`]; Cancel emits
//! [`FormEvent::Cancel`]. The owning `Workspace` subscribes, persists, and
//! opens a tab.
//!
//! Only the dialog body lives here; `Workspace` is responsible for the
//! full-screen backdrop (`occlude()`) that makes it modal.
//!
//! Notes / current limitations:
//! - The engine kind is fixed to MySQL for now (PG/Redis reserved); a real
//!   selector is a follow-up.
//! - The password field shows plaintext (no masking yet), matching the
//!   plaintext-storage decision in [`crate::app::config`].
//! - All inputs share the `"QuillInput"` key context, so Enter in any field
//!   emits the input's `Submit`; we don't wire that to Save here to avoid
//!   accidental submits — use the Save button.

use gpui::{
    div, prelude::*, px, rgb, Context, Entity, EventEmitter, SharedString, Window,
};

use crate::app::config::ConnectionConfig;
use crate::datasource::DbKind;
use crate::ui::text_input::TextInput;
use crate::ui::theme;

/// Emitted by the form.
#[derive(Debug, Clone)]
pub enum FormEvent {
    Save(ConnectionConfig),
    Cancel,
}

pub struct ConnectionForm {
    name: Entity<TextInput>,
    host: Entity<TextInput>,
    port: Entity<TextInput>,
    user: Entity<TextInput>,
    password: Entity<TextInput>,
    database: Entity<TextInput>,
    kind: DbKind,
    error: Option<SharedString>,
}

impl EventEmitter<FormEvent> for ConnectionForm {}

impl ConnectionForm {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mk = |cx: &mut Context<Self>, placeholder: &str, seed: &str| {
            let placeholder = placeholder.to_string();
            let seed = seed.to_string();
            cx.new(|cx| {
                let mut input = TextInput::new(cx, placeholder);
                if !seed.is_empty() {
                    input.set_content(seed, cx);
                }
                input
            })
        };

        Self {
            name: mk(cx, "My connection", "Local MySQL"),
            host: mk(cx, "127.0.0.1", "127.0.0.1"),
            port: mk(cx, "3306", "3306"),
            user: mk(cx, "root", "root"),
            password: mk(cx, "password", ""),
            database: mk(cx, "database (optional)", ""),
            kind: DbKind::Mysql,
            error: None,
        }
    }

    /// Read, validate, and emit Save — or set `error` and re-render.
    fn submit(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).content().to_string();
        let host = self.host.read(cx).content().to_string();
        let port_raw = self.port.read(cx).content().to_string();
        let user = self.user.read(cx).content().to_string();
        let password = self.password.read(cx).content().to_string();
        let database = self.database.read(cx).content().to_string();

        if host.trim().is_empty() {
            self.set_error("Host is required", cx);
            return;
        }
        let port: u16 = match port_raw.trim().parse() {
            Ok(p) => p,
            Err(_) => {
                self.set_error("Port must be a number (1–65535)", cx);
                return;
            }
        };
        let name = if name.trim().is_empty() {
            format!("{}:{}", host.trim(), port)
        } else {
            name.trim().to_string()
        };

        let config = ConnectionConfig::new(
            name,
            self.kind,
            host.trim().to_string(),
            port,
            user,
            password,
            database.trim().to_string(),
        );
        cx.emit(FormEvent::Save(config));
    }

    fn set_error(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(msg.into());
        cx.notify();
    }

    /// Show a connection error reported by the caller (e.g. bad credentials)
    /// without rebuilding the form.
    pub fn show_error(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.set_error(msg, cx);
    }

    fn labeled_field(
        &self,
        label: &str,
        input: &Entity<TextInput>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(theme::PAD_XS))
            .child(
                div()
                    .text_color(rgb(theme::TEXT_DIM))
                    .text_size(px(theme::TEXT_SIZE_XS))
                    .child(SharedString::from(label.to_string())),
            )
            .child(
                div()
                    .h(px(theme::ROW_HEIGHT + 4.))
                    .px(px(theme::PAD))
                    .flex()
                    .items_center()
                    .bg(rgb(theme::BG_DEEP))
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .rounded(px(theme::RADIUS))
                    .font_family(theme::FONT_MONO)
                    .text_size(px(theme::TEXT_SIZE_SM))
                    .child(input.clone()),
            )
    }

    fn button(
        &self,
        id: &'static str,
        label: &'static str,
        bg: u32,
        fg: u32,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .px(px(theme::PAD_XL))
            .py(px(theme::PAD_SM))
            .rounded(px(theme::RADIUS))
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .text_size(px(theme::TEXT_SIZE_SM))
            .hover(|s| s.opacity(0.85))
            .active(|s| s.opacity(0.7))
            .on_click(cx.listener(move |this, _ev, _window, cx| on_click(this, cx)))
            .child(label)
    }
}

impl Render for ConnectionForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut dialog = div()
            .w(px(440.))
            .flex()
            .flex_col()
            .gap(px(theme::PAD_LG))
            .p(px(theme::PAD_XL))
            .bg(rgb(theme::BG_PANEL))
            .border_1()
            .border_color(rgb(theme::BORDER))
            .rounded(px(theme::RADIUS_LG))
            .shadow_lg()
            // Title row with a divider beneath it.
            .child(
                div()
                    .pb(px(theme::PAD))
                    .border_b_1()
                    .border_color(rgb(theme::BORDER))
                    .text_color(rgb(theme::TEXT))
                    .text_size(px(theme::TEXT_SIZE))
                    .child(SharedString::from(format!(
                        "New {} connection",
                        self.kind.label()
                    ))),
            )
            .child(self.labeled_field("Name", &self.name))
            .child(self.labeled_field("Host", &self.host))
            .child(self.labeled_field("Port", &self.port))
            .child(self.labeled_field("User", &self.user))
            .child(self.labeled_field("Password", &self.password))
            .child(self.labeled_field("Database", &self.database));

        if let Some(err) = &self.error {
            dialog = dialog.child(
                div()
                    .text_color(rgb(theme::DANGER))
                    .text_size(px(theme::TEXT_SIZE_SM))
                    .child(err.clone()),
            );
        }

        dialog.child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap(px(theme::PAD))
                .pt(px(theme::PAD))
                .border_t_1()
                .border_color(rgb(theme::BORDER))
                .child(self.button(
                    "form-cancel",
                    "Cancel",
                    theme::SURFACE,
                    theme::TEXT,
                    cx,
                    |_this, cx| cx.emit(FormEvent::Cancel),
                ))
                .child(self.button(
                    "form-save",
                    "Connect",
                    theme::ACCENT,
                    theme::BG_DEEP,
                    cx,
                    |this, cx| this.submit(cx),
                )),
        )
    }
}
