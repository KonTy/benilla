//! The `DBFilesClient` sqlite file: an optional, faster home for the DBC tables. A `Chain`
//! opened on a `Data` folder adopts it when present, and DBC loads prefer the matching table
//! over the patch chain's file, falling back to the file when the table is missing or does not
//! match the reader's schema.
//!
//! A table is named for its DBC file without the `.dbc`. Its first column, `rec`, holds each
//! record's raw bytes at the file's record size; every other column is named `c<k>` for the
//! 4-byte slot it carries and holds that slot's string. A DBC's string offsets mean nothing
//! outside their file, so the reader rebuilds the string block from these columns in row
//! order, column order: a value renamed in the table is picked up as-is. `dump_directory`
//! builds the file from a folder of extracted DBCs, one table per file.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use benilla_dbc::{FieldType, Record, RecordSet, Schema, StringRef, Value};
use rusqlite::{Connection, ToSql};

use crate::Chain;

/// The sqlite file's name inside a `Data` folder.
pub const FILE_NAME: &str = "DBFilesClient";

/// The `Data` folder's `DBFilesClient` file: a table named for each DBC file, read as record
/// sets. `Send + Sync`; a read locks one connection, and DBC loads are a startup affair.
pub struct DbFiles {
    /// The tables, by lowercased name: lookup is case-insensitive, as MPQ names are.
    tables: HashMap<String, String>,
    inner: Mutex<Inner>,
}

struct Inner {
    conn: Connection,
    /// Per table: the layout verified once, or a refusal.
    layout: HashMap<String, Layout>,
    /// DBC headers read from the chain, per table: `None` when the file is in no archive.
    headers: HashMap<String, Option<DbcHeader>>,
    /// `(table, field count, string slots)` refused on a mismatch: rescan it, nothing changes.
    refused: HashSet<String>,
}

/// The DBC file's 20-byte header counts, the two the table checks against.
#[derive(Clone, Copy, Debug)]
struct DbcHeader {
    field_count: u32,
    record_size: u32,
}

/// A verified table's string columns, in column order: their slot indices.
#[derive(Clone)]
enum Layout {
    Ok(Vec<u32>),
    Bad,
}

/// The table name behind a DBC path: `DBFilesClient\<name>.dbc`, either separator, any case,
/// as MPQ name comparison is.
pub fn dbc_table_name(path: &str) -> Option<&str> {
    let (dir, file) = path.rsplit_once(['/', '\\'])?;
    if !dir.eq_ignore_ascii_case("DBFilesClient") {
        return None;
    }
    let stem_len = file.to_ascii_lowercase().strip_suffix(".dbc")?.len();
    file.get(..stem_len).filter(|s| !s.is_empty())
}

impl DbFiles {
    /// Open the sqlite file at `path`; an error means it is not a usable database.
    pub fn open(path: &Path) -> Result<Self> {
        let conn =
            Connection::open(path).with_context(|| format!("opening sqlite {}", path.display()))?;
        // A file that is not a database fails here, not at the first table read.
        conn.execute_batch("SELECT 1 FROM sqlite_master LIMIT 1")
            .map_err(|e| anyhow!("{} is not a sqlite database: {e}", path.display()))?;
        // Its own block: `stmt` borrows `conn` and must drop before the move below.
        let tables = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
                .context("listing tables")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .context("listing tables")?;
            let mut tables = HashMap::new();
            for row in rows {
                let name = row.context("listing tables")?;
                tables.insert(name.to_ascii_lowercase(), name);
            }
            tables
        };
        Ok(Self {
            tables,
            inner: Mutex::new(Inner {
                conn,
                layout: HashMap::new(),
                headers: HashMap::new(),
                refused: HashSet::new(),
            }),
        })
    }

    /// Whether a DBC name has a table here, case-insensitive.
    pub fn has_table(&self, stem: &str) -> bool {
        self.tables.contains_key(&stem.to_ascii_lowercase())
    }

    /// The table behind a DBC path, decoded against `schema`; `None` when the table is missing
    /// or does not match, so the caller reads the DBC file as usual.
    pub fn record_set(&self, chain: &Chain, dbc_path: &str, schema: &Schema) -> Option<RecordSet> {
        let stem = dbc_table_name(dbc_path)?;
        let table = self.tables.get(&stem.to_ascii_lowercase())?;
        let header = {
            let mut inner = self.inner.lock().ok()?;
            *inner
                .headers
                .entry(table.clone())
                .or_insert_with(|| chain_header(chain, dbc_path))
        };
        self.decode(table, schema, header)
    }
    /// A table decoded against `schema`: `header` is the DBC file's counts when the file is in
    /// the chain, `None` when it is not (the table then stands alone and its rows carry the
    /// record size). Any mismatch is a `None`, not an error: the file wins.
    fn decode(&self, table: &str, schema: &Schema, header: Option<DbcHeader>) -> Option<RecordSet> {
        let fc = schema.expanded_len();

        // One type per 4-byte slot, arrays expanded.
        let mut types: Vec<FieldType> = Vec::with_capacity(fc);
        for field in &schema.fields {
            types.extend(std::iter::repeat_n(field.ty, field.count));
        }
        let mut string_slots: Vec<u32> = Vec::new();
        for (i, ty) in types.iter().enumerate() {
            if *ty == FieldType::String {
                string_slots.push(i as u32);
            }
        }
        let key = format!(
            "{table}::{}::{}",
            fc,
            string_slots
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );

        // Through a local `&mut Inner`, so field borrows (`conn` for the scan, `refused` on a
        // refusal) don't cross: a guard's auto-deref borrows the whole pointee.
        let mut guard = self.inner.lock().ok()?;
        let inner = &mut *guard;
        if inner.refused.contains(&key) {
            return None;
        }

        // The layout, verified once per table: `rec` first, then one `c<k>` string column per
        // slot the dumper found.
        let slots = match inner.layout.get(table) {
            Some(Layout::Ok(slots)) => slots.clone(),
            Some(Layout::Bad) => {
                return None;
            }
            None => {
                let layout = probe_layout(&inner.conn, table);
                let slots = match &layout {
                    Layout::Ok(slots) => slots.clone(),
                    Layout::Bad => {
                        return None;
                    }
                };
                inner.layout.insert(table.to_string(), layout);
                slots
            }
        };

        // A `c<k>` column on a slot the schema reads as numeric is harmless: `rec` carries the
        // slot's original bits, which the reader takes instead. A String slot with no column
        // is a zero column, checked per row. Columns past the schema's slots are ignored.
        let mut slot_to_pos: HashMap<u32, usize> = HashMap::with_capacity(slots.len());
        let mut cols: Vec<String> = Vec::with_capacity(slots.len());
        for &k in &slots {
            if k < fc as u32 && types[k as usize] == FieldType::String {
                slot_to_pos.insert(k, cols.len());
                cols.push(format!("\"c{k}\""));
            }
        }

        // The file's counts, when it is in the chain: a different field count is another
        // build's table, and a record smaller than its slots is one the table cannot hold.
        let expected_rs = match header {
            Some(h) => {
                if h.field_count as usize != fc || (h.record_size as u64) < (fc as u64) * 4 {
                    inner.refused.insert(key.clone());
                    return None;
                }
                Some(h.record_size)
            }
            None => None,
        };

        let quoted = table.replace('"', "\"\"");
        let sql = if cols.is_empty() {
            format!("SELECT \"rec\" FROM \"{}\"", quoted)
        } else {
            format!("SELECT \"rec\", {} FROM \"{}\"", cols.join(", "), quoted)
        };
        // A field-level borrow, so the loop can still update `inner.refused`.
        let conn = &inner.conn;
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => {
                inner.refused.insert(key.clone());
                return None;
            }
        };
        let mut rows = match stmt.query([]) {
            Ok(r) => r,
            Err(_) => {
                inner.refused.insert(key.clone());
                return None;
            }
        };

        // The rebuilt string block: offset 0 is the empty string, as the shipped blocks lead
        // with it, then every string cell in row order, column order, NUL-terminated.
        let mut block: Vec<u8> = vec![0u8];
        let mut records: Vec<Record> = Vec::new();
        let mut row_rs: Option<u32> = None;

        loop {
            let row = match rows.next() {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(_) => {
                    inner.refused.insert(key.clone());
                    return None;
                }
            };
            let rec: &[u8] = match row.get_ref(0).ok().and_then(|v| v.as_blob().ok()) {
                Some(rec) => rec,
                None => {
                    inner.refused.insert(key.clone());
                    return None;
                }
            };
            if rec.len() > u32::MAX as usize {
                inner.refused.insert(key.clone());
                return None;
            }
            let rec_len = rec.len() as u32;
            match (expected_rs, row_rs) {
                (Some(expected), _) if rec_len != expected => {
                    inner.refused.insert(key.clone());
                    return None;
                }
                (None, Some(expected)) if rec_len != expected => {
                    inner.refused.insert(key.clone());
                    return None;
                }
                (None, None) => {
                    // No file in the chain to carry the record size: the first row sets it,
                    // and a record smaller than its slots is one the table cannot hold.
                    if rec_len < fc as u32 * 4 {
                        inner.refused.insert(key.clone());
                        return None;
                    }
                    row_rs = Some(rec_len);
                }
                _ => {}
            }

            let mut values = Vec::with_capacity(fc);
            for (i, ty) in types.iter().enumerate() {
                let off = i * 4;
                match ty {
                    FieldType::String => match slot_to_pos.get(&(i as u32)) {
                        Some(&pos) => {
                            let bytes: &[u8] =
                                match row.get_ref(pos + 1).ok().and_then(|v| v.as_bytes().ok()) {
                                    Some(bytes) => bytes,
                                    None => {
                                        inner.refused.insert(key.clone());
                                        return None;
                                    }
                                };
                            let at = block.len() as u32;
                            block.extend_from_slice(bytes);
                            block.push(0);
                            values.push(Value::StringRef(StringRef(at)));
                        }
                        None => {
                            // A zero string column: offset 0, the empty string.
                            let bits = u32::from_le_bytes([
                                rec[off],
                                rec[off + 1],
                                rec[off + 2],
                                rec[off + 3],
                            ]);
                            if bits != 0 {
                                inner.refused.insert(key.clone());
                                return None;
                            }
                            values.push(Value::StringRef(StringRef(0)));
                        }
                    },
                    _ => {
                        let bits = u32::from_le_bytes([
                            rec[off],
                            rec[off + 1],
                            rec[off + 2],
                            rec[off + 3],
                        ]);
                        values.push(match ty {
                            FieldType::UInt32 => Value::UInt32(bits),
                            FieldType::Int32 => Value::Int32(bits as i32),
                            FieldType::Float32 => Value::Float32(f32::from_bits(bits)),
                            FieldType::String => unreachable!("a String slot, handled above"),
                        });
                    }
                }
            }
            records.push(Record::new(values));
        }

        // One field name per slot, arrays expanded, as the parser names them.
        let mut names: Vec<String> = Vec::with_capacity(fc);
        for field in &schema.fields {
            for k in 0..field.count {
                names.push(if field.count == 1 {
                    field.name.clone()
                } else {
                    format!("{}[{k}]", field.name)
                });
            }
        }
        Some(RecordSet::new(names, records, block))
    }
}
/// A table's layout from `PRAGMA table_info`: `rec` BLOB first, then `c<k>` columns of TEXT or
/// BLOB, the slots unique; anything else is `Bad`.
fn probe_layout(conn: &Connection, table: &str) -> Layout {
    let quoted = table.replace('"', "\"\"");
    let sql = format!("PRAGMA table_info(\"{quoted}\")");
    let mut stmt = match conn.prepare(&sql) {
        Ok(stmt) => stmt,
        Err(_) => return Layout::Bad,
    };
    let mut rows = match stmt.query([]) {
        Ok(rows) => rows,
        Err(_) => return Layout::Bad,
    };
    let mut slots: Vec<u32> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut idx = 0usize;
    while let Ok(Some(row)) = rows.next() {
        let name: String = match row.get(1) {
            Ok(name) => name,
            Err(_) => return Layout::Bad,
        };
        let ty: String = match row.get(2) {
            Ok(ty) => ty,
            Err(_) => return Layout::Bad,
        };
        let ty = ty.to_ascii_uppercase();
        if idx == 0 {
            if !name.eq_ignore_ascii_case("rec") || ty != "BLOB" {
                return Layout::Bad;
            }
        } else {
            match name.strip_prefix('c').and_then(parse_slot_index) {
                Some(k) if (ty == "TEXT" || ty == "BLOB") && seen.insert(k) => slots.push(k),
                _ => {
                    return Layout::Bad;
                }
            }
        }
        idx += 1;
    }
    if idx == 0 {
        Layout::Bad
    } else {
        Layout::Ok(slots)
    }
}

/// A `c<k>` slot index: decimal, no leading zero.
fn parse_slot_index(s: &str) -> Option<u32> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    s.parse().ok()
}

/// A DBC's 20-byte header from the chain, when the file is there at all: a file in no archive,
/// a tombstone, a short read and a bad magic are all `None` — the table then stands alone.
fn chain_header(chain: &Chain, path: &str) -> Option<DbcHeader> {
    use std::io::Read;
    let archive = chain.archive_for(path).ok()?;
    let mut file = archive.open_file(path).ok()?;
    let mut buf = [0u8; 20];
    file.read_exact(&mut buf).ok()?;
    if &buf[0..4] != b"WDBC" {
        return None;
    }
    Some(DbcHeader {
        field_count: u32::from_le_bytes(buf[8..12].try_into().ok()?),
        record_size: u32::from_le_bytes(buf[12..16].try_into().ok()?),
    })
}
// ── The dumper: a folder of extracted DBCs into one file, one table per DBC. ─────────────────

/// A dump's tally.
#[derive(Debug)]
pub struct DumpSummary {
    pub tables: usize,
    pub records: u64,
    /// Files that were not a WDBC (empty extraction artifacts): no table for them, so the
    /// client reads those DBCs from the patch chain.
    pub skipped: Vec<String>,
    pub out_bytes: u64,
}

/// Dump a folder of extracted DBC files into one sqlite file: one table per file, named for the
/// file without its `.dbc`. A file that is not a WDBC is skipped by name; nothing is written
/// for it, so the client reads that DBC from the patch chain as before.
pub fn dump_directory(db_dir: &Path, out: &Path) -> Result<DumpSummary> {
    let names: Vec<String> = std::fs::read_dir(db_dir)
        .with_context(|| format!("listing {}", db_dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.to_ascii_lowercase().ends_with(".dbc"))
        .collect();
    let mut names = names;
    names.sort();

    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("making {}", parent.display()))?;
    }
    let conn =
        Connection::open(out).with_context(|| format!("opening sqlite {}", out.display()))?;
    conn.execute_batch("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF;")?;

    let mut summary = DumpSummary {
        tables: 0,
        records: 0,
        skipped: Vec::new(),
        out_bytes: 0,
    };
    for name in &names {
        let bytes = std::fs::read(db_dir.join(name)).with_context(|| format!("reading {name}"))?;
        let (rc, fc, rs, ssz) = match wdbc_header(&bytes) {
            Some(h) => h,
            None => {
                eprintln!(
                    "dbfiles: skipping {name}: not a WDBC ({} bytes)",
                    bytes.len()
                );
                summary.skipped.push(name.clone());
                continue;
            }
        };
        let body = &bytes[20..];
        let rec_len = (rc as usize)
            .checked_mul(rs as usize)
            .and_then(|l| l.checked_add(ssz as usize))
            .ok_or_else(|| anyhow!("{name}: the header counts overflow"))?;
        if body.len() < rec_len {
            eprintln!(
                "dbfiles: skipping {name}: truncated ({} bytes, {} needed)",
                body.len(),
                rec_len
            );
            summary.skipped.push(name.clone());
            continue;
        }
        let rec_bytes = (rc as usize) * (rs as usize);
        let records_bytes = &body[..rec_bytes];
        let strings = &body[rec_bytes..rec_len];
        dump_table(&conn, name, rc, fc, rs, records_bytes, strings)?;
        summary.tables += 1;
        summary.records += rc as u64;
        eprintln!("dbfiles: {name}: {rc} records, {fc} fields");
    }
    summary.out_bytes = std::fs::metadata(out)
        .with_context(|| format!("sizing {}", out.display()))?
        .len();
    Ok(summary)
}

/// A WDBC's four header counts, from a whole file.
fn wdbc_header(bytes: &[u8]) -> Option<(u32, u32, u32, u32)> {
    if bytes.len() < 20 || &bytes[0..4] != b"WDBC" {
        return None;
    }
    let rd = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    Some((rd(4), rd(8), rd(12), rd(16)))
}

/// One DBC's table: `rec` holds each record's original bytes, untouched, and each string slot
/// gets a `c<k>` column carrying its string; the reader rebuilds the string block from the
/// columns it takes as strings and reads every other slot out of `rec`.
fn dump_table(
    conn: &Connection,
    name: &str,
    rc: u32,
    fc: u32,
    rs: u32,
    records_bytes: &[u8],
    strings: &[u8],
) -> Result<()> {
    let table = name
        .strip_suffix(".dbc")
        .ok_or_else(|| anyhow!("{name}: no .dbc suffix"))?;
    let is_str = classify_string_slots(records_bytes, rc, fc, rs, strings);
    let string_cols: Vec<u32> = (0..fc as usize)
        .filter(|c| is_str[*c])
        .map(|c| c as u32)
        .collect();

    // Per string column: its cell bytes per row, and whether the column is all valid UTF-8
    // (stored TEXT) or not (stored BLOB).
    let mut col_cells: Vec<Vec<Vec<u8>>> = Vec::with_capacity(string_cols.len());
    let mut col_utf8: Vec<bool> = Vec::with_capacity(string_cols.len());
    for &c in &string_cols {
        let mut cells = Vec::with_capacity(rc as usize);
        let mut utf8 = true;
        for r in 0..rc as usize {
            let off = slot_u32(records_bytes, r, rs as usize, c as usize) as usize;
            let s = string_at(strings, off);
            if utf8 && std::str::from_utf8(s).is_err() {
                utf8 = false;
            }
            cells.push(s.to_vec());
        }
        col_cells.push(cells);
        col_utf8.push(utf8);
    }

    let mut cols = vec!["rec BLOB".to_string()];
    for (j, &c) in string_cols.iter().enumerate() {
        cols.push(format!(
            "c{c} {}",
            if col_utf8[j] { "TEXT" } else { "BLOB" }
        ));
    }
    let quoted = table.replace('"', "\"\"");
    conn.execute_batch(&format!(
        "DROP TABLE IF EXISTS \"{quoted}\"; CREATE TABLE \"{quoted}\" ({});",
        cols.join(", ")
    ))
    .with_context(|| format!("creating table {quoted}"))?;

    let ins = format!(
        "INSERT INTO \"{quoted}\" VALUES ({});",
        (0..=string_cols.len())
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut stmt = conn
        .prepare(&ins)
        .with_context(|| format!("preparing {quoted}"))?;
    conn.execute_batch("BEGIN;")?;

    let stride = rs as usize;
    for r in 0..rc as usize {
        // The record's original bytes, untouched: a reader reads a slot it does not take from
        // a string column out of here, so nothing may be rewritten.
        let rec = records_bytes[r * stride..(r + 1) * stride].to_vec();
        let mut boxes: Vec<Box<dyn ToSql>> = Vec::with_capacity(1 + string_cols.len());
        for j in 0..string_cols.len() {
            let cells = &col_cells[j][r];
            if col_utf8[j] {
                let s = std::str::from_utf8(cells).expect("checked utf8 per column");
                boxes.push(Box::new(s.to_string()));
            } else {
                boxes.push(Box::new(cells.clone()));
            }
        }
        boxes.insert(0, Box::new(rec));
        let refs: Vec<&dyn ToSql> = boxes.iter().map(|b| b.as_ref()).collect();
        stmt.execute(refs.as_slice())
            .with_context(|| format!("inserting {quoted} row {r}"))?;
    }
    conn.execute_batch("COMMIT;").map_err(|e| {
        let _ = conn.execute_batch("ROLLBACK;");
        anyhow!("committing {quoted}: {e}")
    })?;
    Ok(())
}

/// A slot's 4-byte value the way the parser reads it: `r * record_size + i * 4`, past the end
/// reading as zero.
fn slot_u32(records: &[u8], r: usize, record_size: usize, i: usize) -> u32 {
    let base = r * record_size + i * 4;
    let mut bytes = [0u8; 4];
    for (k, slot) in bytes.iter_mut().enumerate() {
        if let Some(v) = base.checked_add(k).and_then(|idx| records.get(idx)) {
            *slot = *v;
        }
    }
    u32::from_le_bytes(bytes)
}

/// The slots that hold string offsets: every value a string start (0, a position after a NUL,
/// or the block end) and the slot not all zero. A real string slot can never fail this — its
/// values are the offsets the exporter wrote — so a miss is only ever an unused string slot;
/// a numeric slot whose values all land on starts is the one false-positive shape, and the
/// reader refuses a table whose string columns are not String slots in its schema.
pub fn classify_string_slots(
    records: &[u8],
    record_count: u32,
    field_count: u32,
    record_size: u32,
    strings: &[u8],
) -> Vec<bool> {
    let ssz = strings.len();
    let mut starts = vec![false; ssz + 1];
    starts[0] = true;
    starts[ssz] = true;
    for (i, &b) in strings.iter().enumerate() {
        if b == 0 && i + 1 < ssz {
            starts[i + 1] = true;
        }
    }
    (0..field_count as usize)
        .map(|c| {
            let mut all_zero = true;
            for r in 0..record_count as usize {
                let v = slot_u32(records, r, record_size as usize, c) as usize;
                if v != 0 {
                    all_zero = false;
                }
                if v > ssz || !starts[v] {
                    return false;
                }
            }
            !all_zero
        })
        .collect()
}

/// The string at an offset: to its NUL, or the block end. An offset past the end is empty.
fn string_at(strings: &[u8], off: usize) -> &[u8] {
    let Some(rest) = strings.get(off..) else {
        return b"";
    };
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    &rest[..end]
}
#[cfg(test)]
mod tests {
    use super::*;
    use benilla_dbc::{DbcParser, SchemaField};
    use std::io::Cursor;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A unique temp dir per test, so parallel tests never collide.
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "benilla_dbfiles_test_{}_{}_{}",
            std::process::id(),
            n,
            tag
        ));
        std::fs::create_dir_all(&dir).expect("make temp dir");
        dir
    }

    /// A WDBC: id u32, name string, val i32, other string (all zero); 4 slots, 16-byte records.
    fn synthetic_wdbc() -> Vec<u8> {
        // offsets: "" @0, "Alpha" @1, "Beta" @7
        let strings = b"\0Alpha\0Beta\0";
        let rec = |id: u32, name: u32, val: i32| {
            let mut b = Vec::new();
            b.extend_from_slice(&id.to_le_bytes());
            b.extend_from_slice(&name.to_le_bytes());
            b.extend_from_slice(&val.to_le_bytes());
            b.extend_from_slice(&0u32.to_le_bytes()); // other: always empty
            b
        };
        let mut records = Vec::new();
        records.extend(rec(1, 1, -5));
        records.extend(rec(2, 7, 40));
        records.extend(rec(3, 1, 0));
        let mut b = Vec::new();
        b.extend_from_slice(b"WDBC");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        b.extend_from_slice(&records);
        b.extend_from_slice(strings);
        b
    }

    fn schema() -> Schema {
        let mut s = Schema::new("Test");
        s.add_field(SchemaField::new("id", FieldType::UInt32));
        s.add_field(SchemaField::new("name", FieldType::String));
        s.add_field(SchemaField::new("val", FieldType::Int32));
        s.add_field(SchemaField::new("other", FieldType::String));
        s
    }

    /// The synthetic DBC dumped into a table, the file opened around it.
    fn dumped() -> (std::path::PathBuf, DbFiles) {
        let dir = temp_dir("synthetic");
        std::fs::write(dir.join("Test.dbc"), synthetic_wdbc()).expect("write DBC");
        let out = dir.join("DBFilesClient");
        let summary = dump_directory(&dir, &out).expect("dump");
        assert_eq!(summary.tables, 1, "one table, nothing skipped");
        (dir, DbFiles::open(&out).expect("open"))
    }

    /// The synthetic header, as `chain_header` would report it.
    fn header() -> Option<DbcHeader> {
        Some(DbcHeader {
            field_count: 4,
            record_size: 16,
        })
    }

    #[test]
    fn dbc_table_name_parses() {
        assert_eq!(dbc_table_name("DBFilesClient\\Spell.dbc"), Some("Spell"));
        assert_eq!(
            dbc_table_name("DBFilesClient/ChrRaces.DBC"),
            Some("ChrRaces")
        );
        assert_eq!(dbc_table_name("dbfilesclient\\C.cfg.dbc"), Some("C.cfg"));
        assert_eq!(dbc_table_name("DBFilesClient\\Spell"), None);
        assert_eq!(dbc_table_name("DBFilesClient\\Spell.dbc\\x"), None);
        assert_eq!(dbc_table_name("DBFilesClient\\dbc"), None);
        assert_eq!(dbc_table_name("Interface\\Fonts\\x.dbc"), None);
    }

    #[test]
    fn classify_string_slots_on_the_synthetic() {
        let bytes = synthetic_wdbc();
        let records = &bytes[20..20 + 3 * 16];
        let strings = &bytes[20 + 3 * 16..];
        let is_str = classify_string_slots(records, 3, 4, 16, strings);
        assert_eq!(
            is_str,
            [false, true, false, false],
            "name is a string slot; the all-zero other column is not"
        );
    }

    /// Every slot decodes to what the file holds: numbers as stored, strings as resolved.
    #[test]
    fn roundtrip_matches_the_file_parse() {
        let (dir, db) = dumped();
        let file = DbcParser::parse(&mut Cursor::new(synthetic_wdbc().as_slice()))
            .expect("file header")
            .with_schema(schema().clone())
            .expect("schema")
            .parse_records()
            .expect("file records");
        let table = db
            .decode("Test", &schema(), header())
            .expect("table decodes");
        assert_eq!(file.records().len(), table.records().len());
        for (r, (f, t)) in file.records().iter().zip(table.records()).enumerate() {
            for i in 0..4 {
                match (f.get_value(i), t.get_value(i)) {
                    (Some(Value::UInt32(a)), Some(Value::UInt32(b))) if a == b => {}
                    (Some(Value::Int32(a)), Some(Value::Int32(b))) if a == b => {}
                    (Some(Value::StringRef(a)), Some(Value::StringRef(b))) => {
                        assert_eq!(
                            file.get_string(*a).unwrap(),
                            table.get_string(*b).unwrap(),
                            "record {r} slot {i}"
                        );
                    }
                    other => panic!("record {r} slot {i}: {other:?}"),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A renamed string cell is what the client reads: the block is rebuilt from the table.
    #[test]
    fn edited_string_cell_is_read() {
        let (dir, db) = dumped();
        {
            let conn = Connection::open(dir.join("DBFilesClient")).expect("reopen");
            // The dump inserts in record order, so rowid 2 is the second record (id 2, "Beta").
            conn.execute_batch("UPDATE Test SET c1 = 'Gamma' WHERE rowid = 2;")
                .expect("rename row 2's name");
        }
        let table = db
            .decode("Test", &schema(), header())
            .expect("table decodes");
        let name = match table.records()[1].get_value(1) {
            Some(Value::StringRef(s)) => table.get_string(*s).unwrap().to_string(),
            other => panic!("not a string: {other:?}"),
        };
        assert_eq!(name, "Gamma");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_table_is_none() {
        let (_dir, db) = dumped();
        assert!(!db.has_table("Nope"));
        assert!(db.decode("Nope", &schema(), header()).is_none());
    }

    /// A field count that disagrees with the file header: another build's table; the file wins.
    #[test]
    fn a_field_count_mismatch_is_refused() {
        let (_dir, db) = dumped();
        let wrong = Some(DbcHeader {
            field_count: 5,
            record_size: 20,
        });
        assert!(db.decode("Test", &schema(), wrong).is_none());
    }

    /// A string column the schema does not call a String slot: `rec` keeps the slot's original
    /// bits, so the value reads out of there and the table still decodes.
    #[test]
    fn a_string_column_on_a_numeric_slot_reads_the_record_bits() {
        let (_dir, db) = dumped();
        let mut s = Schema::new("Test");
        s.add_field(SchemaField::new("id", FieldType::UInt32));
        s.add_field(SchemaField::new("name", FieldType::UInt32));
        s.add_field(SchemaField::new("val", FieldType::Int32));
        s.add_field(SchemaField::new("other", FieldType::String));
        let rs = db.decode("Test", &s, header()).expect("the table decodes");
        // Slot 1's original string offsets, read as u32; the zero String slot stays empty.
        assert_eq!(rs.records()[0].get_value(1), Some(&Value::UInt32(1)));
        assert_eq!(rs.records()[1].get_value(1), Some(&Value::UInt32(7)));
        assert_eq!(
            rs.records()[0].get_value(3),
            Some(&Value::StringRef(StringRef(0)))
        );
    }

    /// A String slot with no string column must hold zero bits, a hand edit included.
    #[test]
    fn a_nonzero_bit_in_a_zero_string_column_is_refused() {
        let dir = temp_dir("zero_col");
        let out = dir.join("DBFilesClient");
        {
            let conn = Connection::open(&out).expect("open");
            conn.execute_batch("CREATE TABLE T (rec BLOB);")
                .expect("table");
            // 16 bytes: id 1, name offset 7 (nonzero), val 0, other 0.
            conn.execute_batch("INSERT INTO T VALUES (X'01000000070000000000000000000000');")
                .expect("row");
        }
        let db = DbFiles::open(&out).expect("open");
        assert!(db.decode("T", &schema(), header()).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// No file in the chain: the rows carry the record size and the table decodes.
    #[test]
    fn decodes_without_a_file_header() {
        let (_dir, db) = dumped();
        let table = db.decode("Test", &schema(), None).expect("table decodes");
        assert_eq!(table.records().len(), 3);
    }

    /// No file header and a record smaller than its slots: the table cannot hold it.
    #[test]
    fn a_record_smaller_than_its_slots_is_refused() {
        let dir = temp_dir("small_rec");
        let out = dir.join("DBFilesClient");
        {
            let conn = Connection::open(&out).expect("open");
            conn.execute_batch("CREATE TABLE T (rec BLOB);")
                .expect("table");
            conn.execute_batch("INSERT INTO T VALUES (X'0100000000000000');")
                .expect("row");
        }
        let db = DbFiles::open(&out).expect("open");
        assert!(db.decode("T", &schema(), None).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A table without the `rec` lead column is not a layout at all.
    #[test]
    fn a_wrong_layout_is_refused() {
        let dir = temp_dir("layout");
        let out = dir.join("DBFilesClient");
        {
            let conn = Connection::open(&out).expect("open");
            conn.execute_batch("CREATE TABLE T (a INTEGER);")
                .expect("table");
        }
        let db = DbFiles::open(&out).expect("open");
        assert!(db.decode("T", &schema(), None).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every table in the install's `DBFilesClient` decodes to what the patch chain's DBC file
    /// holds, slot for slot: numbers as stored, strings as resolved.
    #[test]
    fn every_table_roundtrips_against_the_patch_chain() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let db_path = data.join(FILE_NAME);
        if !db_path.is_file() {
            eprintln!("dbfiles round-trip: no DBFilesClient in {}", data.display());
            return;
        }
        let db = DbFiles::open(&db_path).expect("open DBFilesClient");
        let tables: Vec<String> = db.tables.values().cloned().collect();
        assert!(!tables.is_empty(), "the dump wrote no tables");
        for table in tables {
            let path = format!("DBFilesClient\\{table}.dbc");
            let Ok(bytes) = chain.read_file(&path) else {
                eprintln!("dbfiles round-trip: {table} not in the chain; skipped");
                continue;
            };
            let header = chain_header(&chain, &path).expect("a readable header");
            let fc = header.field_count as usize;
            // The schema the dumper's classification implies: the table's string columns as
            // String slots, the rest UInt32 — the same truth both sides read with.
            let slots = {
                let g = db.inner.lock().expect("lock");
                match probe_layout(&g.conn, &table) {
                    Layout::Ok(slots) => slots,
                    Layout::Bad => panic!("{table}: bad layout"),
                }
            };
            let mut s = Schema::new(table.as_str());
            for i in 0..fc {
                let ty = if slots.contains(&(i as u32)) {
                    FieldType::String
                } else {
                    FieldType::UInt32
                };
                s.add_field(SchemaField::new(format!("c{i}"), ty));
            }
            let file = DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
                .expect("file header")
                .with_schema(s.clone())
                .expect("schema matches the file")
                .parse_records()
                .expect("file records");
            let got = match db.decode(&table, &s, Some(header)) {
                Some(rs) => rs,
                None => {
                    assert!(
                        (header.record_size as u64) < (fc as u64) * 4,
                        "{table}: refused without a straddling record"
                    );
                    continue; // a record smaller than its slots: the DBC file wins by design
                }
            };
            assert_eq!(
                file.records().len(),
                got.records().len(),
                "{table}: record count"
            );
            for (r, (f, t)) in file.records().iter().zip(got.records()).enumerate() {
                for i in 0..fc {
                    match (f.get_value(i), t.get_value(i)) {
                        (Some(Value::UInt32(a)), Some(Value::UInt32(b))) => {
                            assert_eq!(*a, *b, "{table} record {r} slot {i}");
                        }
                        (Some(Value::StringRef(a)), Some(Value::StringRef(b))) => {
                            assert_eq!(
                                file.get_string(*a).unwrap(),
                                got.get_string(*b).unwrap(),
                                "{table} record {r} slot {i}"
                            );
                        }
                        other => panic!("{table} record {r} slot {i}: {other:?}"),
                    }
                }
            }
        }
    }
}
