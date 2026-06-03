//! Quill's single source of truth for the visual system: colors, typography,
//! spacing, radii, sizing, and icon glyphs.
//!
//! Every UI module pulls its tokens from here so the look stays consistent and
//! a future "light theme" / re-skin is a one-file change. The aesthetic is
//! compact-professional (TablePlus / DataGrip): small type, dense rows, calm
//! Catppuccin-ish colors.
//!
//! Colors are raw `u32` RGB so callers wrap them with `gpui::rgb(...)`. Sizes
//! are `f32` pixels, wrapped with `gpui::px(...)` at the call site.

// --- colors ----------------------------------------------------------------

// Palette: Dracula (https://draculatheme.com). Official spec colors.
//
/// App background (deepest layer — title/tab bar, gaps). Slightly darker than
/// the panel for separation.
pub const BG_DEEP: u32 = 0x21222c;
/// Panel background (sidebar, editor, results body) — Dracula `background`.
pub const BG_PANEL: u32 = 0x282a36;
/// Slightly raised surface (headers, footers, active tab).
pub const SURFACE: u32 = 0x343746;
/// Alternating row background in tables.
pub const ROW_ALT: u32 = 0x2b2e3b;
/// Hover highlight for clickable rows — Dracula `current line`.
pub const HOVER: u32 = 0x44475a;
/// Selected row/item — a touch brighter than hover so it stays distinct.
pub const SELECTED: u32 = 0x565a75;

/// Accent (selection bar, primary button, focus border) — Dracula `purple`.
pub const ACCENT: u32 = 0xbd93f9;
/// Low-saturation accent for subtle borders/details — Dracula `comment`.
pub const ACCENT_DIM: u32 = 0x6272a4;

/// Hairline borders/dividers — Dracula's deep separator.
pub const BORDER: u32 = 0x191a21;

/// Primary text — Dracula `foreground`.
pub const TEXT: u32 = 0xf8f8f2;
/// Dimmed/secondary text (types, counts, hints) — Dracula `comment`.
pub const TEXT_DIM: u32 = 0x6272a4;
/// NULL / error / destructive — Dracula `red`.
pub const DANGER: u32 = 0xff5555;
/// Success / connected (status dot) — Dracula `green`.
pub const SUCCESS: u32 = 0x50fa7b;

// --- SQL syntax highlighting (Dracula) -------------------------------------

/// Keyword (SELECT, FROM, …) — Dracula `pink`.
pub const SYN_KEYWORD: u32 = 0xff79c6;
/// String literal — Dracula `yellow`.
pub const SYN_STRING: u32 = 0xf1fa8c;
/// Numeric literal — Dracula `purple`.
pub const SYN_NUMBER: u32 = 0xbd93f9;
/// Comment — Dracula `comment`.
pub const SYN_COMMENT: u32 = 0x6272a4;
/// Punctuation — Dracula `cyan`.
pub const SYN_PUNCT: u32 = 0x8be9fd;
/// Identifiers / plain text — foreground.
pub const SYN_IDENT: u32 = 0xf8f8f2;

// --- typography ------------------------------------------------------------

/// UI font (labels, buttons, tree). Empty-ish system default via a common
/// sans stack; gpui falls back to the platform UI font.
pub const FONT_UI: &str = "Helvetica";
/// Monospace font for data, SQL, and table cells (alignment matters there).
pub const FONT_MONO: &str = "Menlo";

/// Base UI text size. Small, for information density.
pub const TEXT_SIZE: f32 = 13.0;
/// Smaller text (column types, secondary labels).
pub const TEXT_SIZE_SM: f32 = 12.0;
/// Smallest text (field captions, footnotes).
pub const TEXT_SIZE_XS: f32 = 11.0;

// --- spacing (consistent rhythm; replaces ad-hoc px_2) ---------------------

pub const PAD_XS: f32 = 4.0;
pub const PAD_SM: f32 = 6.0;
pub const PAD: f32 = 8.0;
pub const PAD_LG: f32 = 12.0;
pub const PAD_XL: f32 = 16.0;

// --- radii -----------------------------------------------------------------

pub const RADIUS_SM: f32 = 4.0;
pub const RADIUS: f32 = 6.0;
pub const RADIUS_LG: f32 = 8.0;

// --- sizing (compact) ------------------------------------------------------

/// Default fixed column width in the results table.
pub const COL_WIDTH: f32 = 160.0;
/// Width of the leading row-number (`#`) column in the results table.
pub const SEQ_WIDTH: f32 = 56.0;
/// Width of the SQL editor's line-number gutter.
pub const GUTTER_WIDTH: f32 = 40.0;
/// Width of the detail panel's narrower line-number gutter.
pub const DETAIL_GUTTER_WIDTH: f32 = 28.0;
/// Standard single-line row height (tree rows, table rows).
pub const ROW_HEIGHT: f32 = 24.0;
/// Tab bar height.
pub const TAB_HEIGHT: f32 = 32.0;
/// Table header height (holds name + type on two lines).
pub const HEADER_HEIGHT: f32 = 34.0;
/// Width of the left sidebar.
pub const SIDEBAR_WIDTH: f32 = 220.0;
/// Height of the SQL editor pane.
pub const EDITOR_HEIGHT: f32 = 110.0;

// --- icons (unified Unicode glyphs; one place to swap later) ---------------

/// Collapsed disclosure triangle (database row, closed).
pub const ICON_CHEVRON: &str = "▸";
/// Expanded disclosure triangle (database row, open).
pub const ICON_CHEVRON_OPEN: &str = "▾";
/// A table.
pub const ICON_TABLE: &str = "▤";
/// A view.
pub const ICON_VIEW: &str = "◫";
/// Connection status dot (filled circle; colored by state).
pub const ICON_DOT: &str = "●";
/// Close (tab, dialog).
pub const ICON_CLOSE: &str = "✕";
/// Run query.
pub const ICON_RUN: &str = "▷";
/// Add / new.
pub const ICON_ADD: &str = "＋";
