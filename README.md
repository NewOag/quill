# Quill

A fast, native database query tool built on [gpui](https://github.com/zed-industries/zed)
(the GPU-accelerated UI framework from Zed). Quill aims to support MySQL,
PostgreSQL, and Redis behind one calm, compact interface.

## Status

Early but usable. Working today:

- **Connection management** — multiple simultaneous connections as tabs, a
  new-connection form, and persistence to `~/.config/quill/connections.json`.
- **Object browser** — database → table/view tree in the sidebar; click a table
  to query it.
- **SQL editor** — a hand-rolled text input with SQL syntax highlighting;
  press Enter (or ▷ Run) to execute.
- **Results grid** — virtualized rows (handles large result sets), horizontal
  scrolling, column sorting, NULL distinction, and per-cell selection.
- **Detail panel** — a resizable panel showing a selected cell's full value with
  format transforms: JSON pretty-print, Base64 decode, URL decode, and Unix
  timestamp → UTC datetime.
- **Dracula** color theme.

MySQL is fully wired; PostgreSQL and Redis are reserved in the data-source
abstraction and not yet connected.

## Architecture

Three layers, GUI-agnostic at the bottom:

- `datasource` — drivers + plain data types (MySQL real; PG/Redis reserved).
- `app` — the `Db` async bridge (tokio ⇄ gpui) and the `Workspace`/`Session` views.
- `ui` — reusable gpui components (sidebar, editor, results table, theme).

## Build & run

Requires a recent Rust toolchain (edition 2024). On macOS the Metal toolchain is
needed once: `xcodebuild -downloadComponent MetalToolchain`.

```sh
cargo run
```

Without a configured connection, Quill starts in an empty state — click **+** to
add one. As a convenience, `QUILL_MYSQL_URL=mysql://user:pass@host:3306/db cargo run`
auto-opens that connection on startup.

```sh
cargo test   # unit tests for SQL building, sorting, format transforms, tokenizer
```
