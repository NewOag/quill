//! The root view: a tab bar over a stack of connection [`Session`]s.
//!
//! `Workspace` is the thin top-level shell. It holds the persisted
//! [`ConnectionStore`], the open tabs (`Vec<Entity<Session>>`), which pane is
//! active, and the lazily-built new-connection form. It does NOT subscribe to
//! any session's panes — each [`Session`] is self-contained. Workspace only
//! reacts to tab-bar clicks and to the form's [`FormEvent`].
//!
//! Modality: the new-connection form is drawn as a centered dialog over a
//! full-screen backdrop that uses `occlude()` to swallow clicks so they don't
//! fall through to the tabs behind it.

use gpui::{
    div, prelude::*, px, rgb, Context, Entity, MouseButton, SharedString, Subscription, Window,
};
use tokio::runtime::Handle;
use uuid::Uuid;

use crate::app::config::{ConnectionConfig, ConnectionStore};
use crate::app::db::Db;
use crate::app::session::{Session, SessionEvent};
use crate::ui::connection_form::{ConnectionForm, FormEvent};
use crate::ui::theme;

/// What the main area below the tab bar shows.
enum Pane {
    /// No tabs open.
    Empty,
    /// The session at this index in `sessions` is active.
    Session(usize),
}

pub struct Workspace {
    /// Tokio handle, cloned into every `Db` we build.
    handle: Handle,
    /// Persisted connections.
    store: ConnectionStore,
    /// Open tabs, 1:1 with currently-open connections.
    sessions: Vec<Entity<Session>>,
    active: Pane,
    /// The new-connection form, present only while it's showing.
    form: Option<Entity<ConnectionForm>>,
    /// Whether we're currently editing an existing connection (vs. creating new).
    editing_id: Option<Uuid>,
    /// Subscription to the form's events (replaced each time a form opens).
    _form_sub: Option<Subscription>,
    /// Subscriptions to each session's events (edit/delete connection).
    _session_subs: Vec<Subscription>,
    /// Whether the saved-connections dropdown is open.
    show_conn_list: bool,
}

impl Workspace {
    pub fn new(
        handle: Handle,
        store: ConnectionStore,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            handle,
            store,
            sessions: Vec::new(),
            active: Pane::Empty,
            show_conn_list: false,
            form: None,
            editing_id: None,
            _form_sub: None,
            _session_subs: Vec::new(),
        };

        // Auto-reopen the last-open connection, if any is recorded and still
        // exists in the store.
        if let Some(id) = this.store.last_open {
            if this.store.find(id).is_some() {
                this.open_connection(id, window, cx);
            }
        }
        this
    }

    /// Inject a connection that isn't persisted (e.g. from `QUILL_MYSQL_URL`)
    /// and open it. The config is added to the in-memory store but only saved
    /// if the user later acts on it.
    pub fn open_ephemeral(
        &mut self,
        config: ConnectionConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = config.id;
        self.store.connections.push(config);
        self.open_connection(id, window, cx);
    }

    // --- connection lifecycle ---------------------------------------------

    /// Open a saved connection as a new tab (or focus it if already open).
    fn open_connection(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.session_index(id, cx) {
            self.activate(i, cx);
            return;
        }
        let Some(config) = self.store.find(id).cloned() else {
            return;
        };

        use crate::datasource::DbKind;
        let build_result = match config.kind {
            DbKind::Mysql => Db::mysql(self.handle.clone(), &config.mysql_url()),
            DbKind::Postgres => Db::postgres(self.handle.clone(), &config.postgres_url()),
            DbKind::Redis => Db::redis(self.handle.clone(), &config.redis_url()),
        };
        let db = match build_result {
            Ok(db) => Some(db),
            Err(e) => {
                // Surface the build error in the form if it's open, else log.
                if let Some(form) = &self.form {
                    form.update(cx, |f, cx| f.show_error(format!("{e:#}"), cx));
                } else {
                    eprintln!("quill: could not open connection: {e:#}");
                }
                return;
            }
        };

        let session = cx.new(|cx| Session::new(id, db, window, cx));
        // Subscribe to session events (edit/delete connection requests).
        let sub = cx.subscribe_in(&session, window, Self::on_session_event);
        self._session_subs.push(sub);
        self.sessions.push(session);
        self.active = Pane::Session(self.sessions.len() - 1);
        self.store.last_open = Some(id);
        self.store.save();
        self.close_form();
        cx.notify();
    }

    fn activate(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.sessions.len() {
            return;
        }
        self.show_conn_list = false;
        self.active = Pane::Session(index);
        self.store.last_open = Some(self.sessions[index].read(cx).config_id);
        self.store.save();
        cx.notify();
    }

    fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.sessions.len() {
            return;
        }
        // Dropping the Entity<Session> cancels its in-flight tasks and releases
        // its panes + subscriptions.
        self.sessions.remove(index);

        self.active = if self.sessions.is_empty() {
            self.store.last_open = None;
            Pane::Empty
        } else {
            let new_index = match &self.active {
                Pane::Session(active) if *active == index => index.min(self.sessions.len() - 1),
                Pane::Session(active) if *active > index => active - 1,
                Pane::Session(active) => *active,
                Pane::Empty => 0,
            };
            self.store.last_open = Some(self.sessions[new_index].read(cx).config_id);
            Pane::Session(new_index)
        };
        self.store.save();
        cx.notify();
    }

    /// Index of an open session for the given config id.
    fn session_index(&self, id: Uuid, cx: &Context<Self>) -> Option<usize> {
        self.sessions
            .iter()
            .position(|s| s.read(cx).config_id == id)
    }

    // --- form -------------------------------------------------------------

    fn open_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = cx.new(|cx| ConnectionForm::new(window, cx));
        // subscribe_in so the handler receives a Window (needed to open a tab).
        self._form_sub = Some(cx.subscribe_in(&form, window, Self::on_form_event));
        self.form = Some(form);
        cx.notify();
    }

    fn close_form(&mut self) {
        self.form = None;
        self._form_sub = None;
    }

    fn on_form_event(
        &mut self,
        _form: &Entity<ConnectionForm>,
        event: &FormEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            FormEvent::Cancel => {
                self.editing_id = None;
                self.close_form();
                cx.notify();
            }
            FormEvent::Save(config) => {
                let config = config.clone();
                let id = config.id;
                if let Some(editing) = self.editing_id.take() {
                    // Update existing connection.
                    self.store.update(editing, config);
                    self.store.save();
                    // Rebuild the tab if it's currently open.
                    if let Some(i) = self.session_index(editing, cx) {
                        self.close_tab(i, cx);
                        self.open_connection(editing, window, cx);
                    }
                    cx.notify();
                } else {
                    // New connection.
                    self.store.connections.push(config);
                    self.store.save();
                    self.open_connection(id, window, cx);
                }
            }
        }
    }

    /// Handle edit/delete requests bubbled up from a Session.
    fn on_session_event(
        &mut self,
        session: &Entity<Session>,
        event: &SessionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let config_id = session.read(cx).config_id;
        match event {
            SessionEvent::EditConnection => {
                let Some(cfg) = self.store.find(config_id).cloned() else { return };
                self.editing_id = Some(config_id);
                let form = cx.new(|cx| ConnectionForm::new(window, cx));
                form.update(cx, |f, cx| f.prefill(&cfg, cx));
                self._form_sub = Some(cx.subscribe_in(&form, window, Self::on_form_event));
                self.form = Some(form);
                cx.notify();
            }
            SessionEvent::DeleteConnection => {
                // Close the tab if open, remove from store.
                if let Some(i) = self.session_index(config_id, cx) {
                    self.close_tab(i, cx);
                }
                self.store.remove(config_id);
                self.store.save();
                cx.notify();
            }
        }
    }

    // --- rendering --------------------------------------------------------

    fn tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut bar = div()
            .h(px(theme::TAB_HEIGHT))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::PAD_XS))
            .px(px(theme::PAD_SM))
            .bg(rgb(theme::BG_DEEP))
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            // Wordmark.
            .child(
                div()
                    .pr(px(theme::PAD_SM))
                    .text_color(rgb(theme::TEXT))
                    .text_size(px(theme::TEXT_SIZE))
                    .child("Quill"),
            );

        let active_index = match &self.active {
            Pane::Session(i) => Some(*i),
            Pane::Empty => None,
        };

        for (i, session) in self.sessions.iter().enumerate() {
            let id = session.read(cx).config_id;
            let name = self
                .store
                .find(id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "(connection)".into());
            let is_active = active_index == Some(i);
            // Active tab: raised surface + a 2px accent bar along the top edge.
            let top_accent = if is_active { theme::ACCENT } else { theme::BG_DEEP };

            bar = bar.child(
                div()
                    .id(SharedString::from(format!("tab-{i}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(theme::PAD_SM))
                    .h(px(theme::TAB_HEIGHT - 6.))
                    .px(px(theme::PAD_LG))
                    .rounded_t(px(theme::RADIUS))
                    .border_t_2()
                    .border_color(rgb(top_accent))
                    .bg(rgb(if is_active { theme::SURFACE } else { theme::BG_PANEL }))
                    .text_size(px(theme::TEXT_SIZE_SM))
                    .text_color(rgb(if is_active { theme::TEXT } else { theme::TEXT_DIM }))
                    .hover(|s| s.bg(rgb(theme::SURFACE)).text_color(rgb(theme::TEXT)))
                    .on_click(cx.listener(move |this, _ev, _window, cx| this.activate(i, cx)))
                    .child(SharedString::from(name))
                    .child(
                        div()
                            .id(SharedString::from(format!("tab-close-{i}")))
                            .flex()
                            .justify_center()
                            .items_center()
                            .size(px(16.))
                            .rounded(px(theme::RADIUS_SM))
                            .text_color(rgb(theme::TEXT_DIM))
                            .text_size(px(theme::TEXT_SIZE_XS))
                            .hover(|s| s.bg(rgb(theme::HOVER)).text_color(rgb(theme::DANGER)))
                            // Close on mouse-down so it wins over the tab's click.
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _ev, _window, cx| this.close_tab(i, cx)),
                            )
                            .child(theme::ICON_CLOSE),
                    ),
            );
        }

        let show_list = self.show_conn_list;
        // Saved-connections dropdown toggle.
        bar = bar.child(
            div()
                .id("tab-conn-list")
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.))
                .h(px(theme::TAB_HEIGHT - 10.))
                .px(px(theme::PAD_SM))
                .rounded(px(theme::RADIUS_SM))
                .text_color(rgb(if show_list { theme::ACCENT } else { theme::TEXT_DIM }))
                .bg(rgb(if show_list { theme::SURFACE } else { theme::BG_PANEL }))
                .text_size(px(theme::TEXT_SIZE_SM))
                .hover(|s| s.bg(rgb(theme::HOVER)).text_color(rgb(theme::TEXT)))
                .on_click(cx.listener(|this, _ev, _window, cx| {
                    this.show_conn_list = !this.show_conn_list;
                    cx.notify();
                }))
                .child("Connections")
                .child(
                    div().text_size(px(9.)).child(
                        if show_list { "▴" } else { "▾" }
                    )
                ),
        );

        // The "+" new-connection button.
        bar.child(
            div()
                .id("tab-new")
                .flex()
                .justify_center()
                .items_center()
                .size(px(theme::TAB_HEIGHT - 10.))
                .rounded(px(theme::RADIUS_SM))
                .text_color(rgb(theme::TEXT_DIM))
                .text_size(px(theme::TEXT_SIZE))
                .hover(|s| s.bg(rgb(theme::HOVER)).text_color(rgb(theme::ACCENT)))
                .on_click(cx.listener(|this, _ev, window, cx| {
                    this.show_conn_list = false;
                    this.open_form(window, cx);
                }))
                .child(theme::ICON_ADD),
        )
    }

    /// Dropdown panel listing all saved connections, anchored below the tab bar.
    fn conn_dropdown(&self, _window: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.show_conn_list {
            return None;
        }
        let connections = self.store.connections.clone();
        if connections.is_empty() {
            return Some(
                div()
                    .absolute()
                    .top(px(theme::TAB_HEIGHT))
                    .right(px(theme::PAD_LG))
                    .w(px(280.))
                    .p(px(theme::PAD_LG))
                    .bg(rgb(theme::SURFACE))
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .rounded(px(theme::RADIUS_LG))
                    .shadow_lg()
                    .text_color(rgb(theme::TEXT_DIM))
                    .text_size(px(theme::TEXT_SIZE_SM))
                    .child("No saved connections — click + to add one")
                    .occlude(),
            );
        }

        let mut list = div()
            .absolute()
            .top(px(theme::TAB_HEIGHT))
            .right(px(theme::PAD_LG))
            .w(px(280.))
            .bg(rgb(theme::SURFACE))
            .border_1()
            .border_color(rgb(theme::BORDER))
            .rounded(px(theme::RADIUS_LG))
            .shadow_lg()
            .occlude()
            // Header
            .child(
                div()
                    .flex_none()
                    .h(px(theme::ROW_HEIGHT))
                    .px(px(theme::PAD_LG))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(theme::BORDER))
                    .text_color(rgb(theme::TEXT_DIM))
                    .text_size(px(theme::TEXT_SIZE_XS))
                    .child(SharedString::from(format!(
                        "{} saved connection{}",
                        connections.len(),
                        if connections.len() == 1 { "" } else { "s" }
                    ))),
            );

        for cfg in connections {
            let id = cfg.id;
            let name = cfg.name.clone();
            let kind_label = match cfg.kind {
                crate::datasource::DbKind::Mysql => "MySQL",
                crate::datasource::DbKind::Postgres => "PG",
                crate::datasource::DbKind::Redis => "Redis",
            };
            let kind_color = match cfg.kind {
                crate::datasource::DbKind::Mysql => theme::SYN_NUMBER,
                crate::datasource::DbKind::Postgres => theme::ACCENT,
                crate::datasource::DbKind::Redis => theme::SYN_KEYWORD,
            };
            let is_open = self.session_index(id, cx).is_some();

            list = list.child(
                div()
                    .id(SharedString::from(format!("clist-{id}")))
                    .h(px(theme::ROW_HEIGHT * 1.4))
                    .px(px(theme::PAD_LG))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(theme::PAD_SM))
                    .border_b_1()
                    .border_color(rgb(theme::BORDER))
                    .hover(|s| s.bg(rgb(theme::HOVER)))
                    .on_click(cx.listener(move |this, _ev, window, cx| {
                        this.show_conn_list = false;
                        this.open_connection(id, window, cx);
                        cx.notify();
                    }))
                    // Kind badge
                    .child(
                        div()
                            .flex_none()
                            .px(px(theme::PAD_XS))
                            .rounded(px(theme::RADIUS_SM))
                            .bg(rgb(theme::BG_DEEP))
                            .text_color(rgb(kind_color))
                            .text_size(px(theme::TEXT_SIZE_XS))
                            .font_family(theme::FONT_MONO)
                            .child(kind_label),
                    )
                    // Connection name
                    .child(
                        div()
                            .flex_grow()
                            .min_w_0()
                            .truncate()
                            .text_size(px(theme::TEXT_SIZE_SM))
                            .text_color(rgb(if is_open { theme::ACCENT } else { theme::TEXT }))
                            .child(SharedString::from(name)),
                    )
                    // "open" indicator
                    .children(is_open.then(||
                        div()
                            .text_size(px(theme::TEXT_SIZE_XS))
                            .text_color(rgb(theme::TEXT_DIM))
                            .child("open"),
                    ))
                    // Delete button — stops propagation so it doesn't also open
                    // the connection.
                    .child(
                        div()
                            .id(SharedString::from(format!("clist-del-{id}")))
                            .flex_none()
                            .px(px(theme::PAD_XS))
                            .rounded(px(theme::RADIUS_SM))
                            .text_color(rgb(theme::TEXT_DIM))
                            .text_size(px(theme::TEXT_SIZE_XS))
                            .hover(|s| s.bg(rgb(theme::BG_DEEP)).text_color(rgb(theme::DANGER)))
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _ev, _w, cx| {
                                    // Close the tab first (if open), then remove.
                                    if let Some(i) = this.session_index(id, cx) {
                                        this.close_tab(i, cx);
                                    }
                                    this.store.remove(id);
                                    this.store.save();
                                    if this.store.connections.is_empty() {
                                        this.show_conn_list = false;
                                    }
                                    cx.notify();
                                }),
                            )
                            .child(theme::ICON_CLOSE),
                    ),
            );
        }

        Some(list)
    }

    fn body(&self) -> impl IntoElement {
        let content = match &self.active {
            Pane::Session(i) if *i < self.sessions.len() => {
                div().size_full().child(self.sessions[*i].clone())
            }
            _ => div()
                .size_full()
                .flex()
                .flex_col()
                .justify_center()
                .items_center()
                .gap(px(theme::PAD_SM))
                .text_color(rgb(theme::TEXT_DIM))
                .child(
                    div()
                        .text_color(rgb(theme::ACCENT_DIM))
                        .text_size(px(32.))
                        .child(theme::ICON_TABLE),
                )
                .child(
                    div()
                        .text_size(px(theme::TEXT_SIZE))
                        .text_color(rgb(theme::TEXT))
                        .child("No connection open"),
                )
                .child(
                    div()
                        .text_size(px(theme::TEXT_SIZE_SM))
                        .child(SharedString::from(format!(
                            "Click {} in the tab bar to add one",
                            theme::ICON_ADD
                        ))),
                ),
        };
        div().flex_grow().child(content)
    }

    /// The modal overlay (backdrop + centered dialog), drawn only when a form
    /// is open. The backdrop `occlude()`s clicks from reaching the tabs.
    fn modal(&self) -> Option<impl IntoElement> {
        self.form.as_ref().map(|form| {
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_center()
                .bg(gpui::rgba(0x00000099))
                .occlude()
                .child(form.clone())
        })
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Root container sets the base font + size + text color so the whole
        // tree inherits the compact-professional defaults.
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::BG_DEEP))
            .font_family(theme::FONT_UI)
            .text_size(px(theme::TEXT_SIZE))
            .text_color(rgb(theme::TEXT))
            .child(self.tab_bar(cx))
            .child(self.body())
            .children(self.conn_dropdown(window, cx))
            .children(self.modal())
    }
}
