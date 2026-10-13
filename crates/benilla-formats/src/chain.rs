//! The vanilla patch chain: a priority-ordered set of MPQ archives, read through `benilla-mpq`. A
//! read resolves a name to the highest-priority archive holding it, so a patch wins; base archives
//! carry no `(listfile)`, so resolution is by name hash.
//!
//! An entry of a `Data` folder that is a folder under a mountable name (a `patch-?.MPQ` of loose
//! files, or the same name with a `.patch` extension) mounts as an archive too, its files at
//! their paths under the folder; see the `Deviation` on [`Chain::open`].

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use benilla_dbc::{RecordSet, Schema};
use benilla_mpq::Archive;

use crate::dbfiles::{DbFiles, FILE_NAME};
use crate::VANILLA_BASE_ORDER;

/// One entry from a chain listing: an internal path and its uncompressed size.
pub struct ChainEntry {
    pub name: String,
    pub size: u64,
}

/// A priority-ordered patch chain of MPQ archives (`Send + Sync`; reads are `&self` and lock-free).
/// A `Data` folder carries its `DBFilesClient` sqlite file too, when present: DBC reads prefer a
/// matching table over the patch chain's file.
pub struct Chain {
    /// Ascending priority: later archives win.
    archives: Vec<Archive>,
    /// The `Data` folder's `DBFilesClient` file: `None` for a single-MPQ chain or an absent file.
    dbfiles: Option<DbFiles>,
}

/// `patch-?.MPQ` as the reference's `FindFirstFileW` glob matches it (template `0x82edbc`, wrapper
/// `0x42ad10`): `?` is exactly one character, any case, so `patch-10.MPQ` never mounts.
fn is_patch_glob_match(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some(mid) = lower
        .strip_prefix("patch-")
        .and_then(|rest| rest.strip_suffix(".mpq"))
    else {
        return false;
    };
    mid.chars().count() == 1
}

/// The mount name of a loose folder: a folder named for an archive with a `.patch` extension
/// (`patch-a.patch`) mounts where its `.mpq` twin (`patch-a.mpq`) would, so it sorts and
/// shadows with the patch order carved for `.mpq` names. Any other folder mounts under its
/// own name.
fn folder_mount_name(actual: &str) -> String {
    match actual.to_ascii_lowercase().strip_suffix(".patch") {
        Some(stem) => format!("{}.mpq", &actual[..stem.len()]),
        None => actual.to_string(),
    }
}

/// The reference's mount order over a `Data` listing, ascending priority (`0x403740`): the ten
/// [`VANILLA_BASE_ORDER`] archives, `patch.MPQ`, every `patch-?.MPQ` by case-folded name (the
/// reference sorts descending with `strnicmp` and walks backwards), then `speech2.MPQ`. Names
/// match case-insensitively and come back as found on disk.
fn mount_order(dir_names: &[String]) -> Vec<String> {
    let find = |want: &str| {
        dir_names
            .iter()
            .find(|n| n.eq_ignore_ascii_case(want))
            .cloned()
    };
    let mut order: Vec<String> = VANILLA_BASE_ORDER.iter().filter_map(|b| find(b)).collect();
    order.extend(find("patch.MPQ"));
    let mut patches: Vec<String> = dir_names
        .iter()
        .filter(|n| is_patch_glob_match(n))
        .cloned()
        .collect();
    patches.sort_by_key(|n| n.to_ascii_lowercase());
    order.extend(patches);
    order.extend(find("speech2.MPQ"));
    order
}

/// The `Data` folder's `DBFilesClient` sqlite file, when one is present: an absent file is a
/// quiet `None`; one that fails to open is skipped with a note, so a broken override never takes
/// the DBCs down with it.
fn open_dbfiles(data_dir: &Path) -> Option<DbFiles> {
    let path = data_dir.join(FILE_NAME);
    if !path.is_file() {
        return None;
    }
    match DbFiles::open(&path) {
        Ok(db) => {
            eprintln!(
                "benilla: adopting {} as a DBC source ({} tables)",
                path.display(),
                db.table_count()
            );
            Some(db)
        }
        Err(e) => {
            eprintln!(
                "benilla: skipping {} as a DBC source: {e:#}",
                path.display()
            );
            None
        }
    }
}

impl Chain {
    /// Open a `Data` directory's archives in [`mount_order`], or a single `.MPQ` file. An entry
    /// that is a folder under a mountable name mounts as an archive of its files, at their paths
    /// under the folder; a `.patch`-named folder (`patch-a.patch`) mounts where its `.mpq` twin
    /// would.
    ///
    /// Deviation: an archive that fails to open is an error, where the reference logs
    /// `"Failed to open archive"` and goes on, because a skipped corrupt archive surfaces only as
    /// missing files far downstream.
    ///
    /// Deviation: a folder under a mountable name mounts as a loose-file archive, where the
    /// 1.12.1 client finds it by the same name glob and its file layer rejects it at the open
    /// (packed MPQs only). A loose folder stands in for a patch without repacking one; the
    /// `.patch` extension is benilla's own, the reference names its patches `.mpq`.
    pub fn open(path: &Path) -> Result<Self> {
        let mut archives = Vec::new();
        if path.is_dir() {
            // Files and folders alike; `path()` follows symlinks (`read_dir`'s file_type doesn't).
            let mut files: Vec<String> = Vec::new();
            // Folders as (mount name, name on disk): a `.patch` folder mounts where its `.mpq`
            // twin would, so its mount name is not its name on disk.
            let mut folders: Vec<(String, String)> = Vec::new();
            for entry in
                std::fs::read_dir(path).with_context(|| format!("listing {}", path.display()))?
            {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => continue,
                };
                let Some(name) = entry.file_name().into_string().ok() else {
                    continue;
                };
                if entry.path().is_file() {
                    files.push(name);
                } else if entry.path().is_dir() {
                    folders.push((folder_mount_name(&name), name));
                }
            }
            // read_dir order is arbitrary; sort so case-variant and `.patch`/`.mpq` ties resolve
            // deterministically (a `.mpq` name beats a `.patch` one of the same stem).
            files.sort();
            folders.sort_by(|a, b| {
                a.0.to_ascii_lowercase()
                    .cmp(&b.0.to_ascii_lowercase())
                    .then_with(|| a.1.cmp(&b.1))
            });
            // Entries that mount to the same name — a file and a folder, case variants or a
            // `.patch`/`.mpq` pair — are one archive: the file, which is what the reference
            // opens; among folders, the first in sort order. `on_disk` keeps each mount name's
            // name on disk for the open below.
            let mut taken: HashSet<String> = files.iter().map(|n| n.to_ascii_lowercase()).collect();
            let mut names: Vec<String> = Vec::with_capacity(files.len() + folders.len());
            let mut on_disk: HashMap<String, String> = HashMap::new();
            for name in &files {
                names.push(name.clone());
                on_disk.insert(name.clone(), name.clone());
            }
            for (mount, actual) in folders {
                let key = mount.to_ascii_lowercase();
                if taken.contains(&key) {
                    continue;
                }
                taken.insert(key);
                names.push(mount.clone());
                on_disk.insert(mount, actual);
            }
            for name in mount_order(&names) {
                let mpq = path.join(&on_disk[&name]);
                let archive = if mpq.is_dir() {
                    Archive::open_dir(&mpq)
                } else {
                    Archive::open(&mpq)
                };
                archives.push(archive.with_context(|| format!("opening {}", mpq.display()))?);
            }
            if archives.is_empty() {
                bail!("no known vanilla archives found in {}", path.display());
            }
            let dbfiles = open_dbfiles(path);
            Ok(Self { archives, dbfiles })
        } else {
            archives.push(
                Archive::open(path).with_context(|| format!("opening MPQ {}", path.display()))?,
            );
            Ok(Self {
                archives,
                dbfiles: None,
            })
        }
    }

    /// The highest-priority archive with an entry for `name`, a delete marker included, as a
    /// tombstone shadows every lower copy: check [`Archive::is_delete_marker`] for a readable file.
    fn resolve(&self, name: &str) -> Option<&Archive> {
        self.archives.iter().rev().find(|a| a.contains(name))
    }

    /// Whether `name` (`/` or `\`, any case) is a readable file, not a delete marker.
    pub fn contains(&self, name: &str) -> bool {
        self.resolve(name)
            .is_some_and(|a| !a.is_delete_marker(name))
    }

    /// The path of the archive `name` resolves to, for debugging and extraction.
    pub fn find_file_archive(&self, name: &str) -> Option<&Path> {
        self.resolve(name).map(|a| a.path())
    }

    /// Read a file by internal path (`/` or `\`) from its winning archive.
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let archive = self.archive_for(name)?;
        archive
            .read_file(name)
            .with_context(|| format!("reading {name} from {}", archive.path().display()))
    }

    /// The archive `name` reads from, as a cheap handle: a caller holding the chain behind a lock
    /// looks the file up under it and reads (`Archive::open_file`) after releasing it. No I/O.
    pub fn archive_for(&self, name: &str) -> Result<Archive> {
        let archive = self
            .resolve(name)
            .ok_or_else(|| anyhow!("file not in patch chain: {name}"))?;
        // A tombstone deletes the path from the composite: not found, never a stale lower copy.
        if archive.is_delete_marker(name) {
            bail!(
                "file deleted from patch chain: {name} (tombstoned by {})",
                archive.path().display()
            );
        }
        Ok(archive.clone())
    }

    /// `&mut` alias of [`Chain::read`] for call sites that thread a `&mut Chain`.
    pub fn read_file(&mut self, name: &str) -> Result<Vec<u8>> {
        self.read(name)
    }

    /// A DBC path's records from the `DBFilesClient` table, when the chain carries one, the table
    /// exists and matches `schema`: `None` otherwise, so the caller reads the DBC file as usual.
    pub fn dbc_record_set(&self, path: &str, schema: &Schema) -> Option<RecordSet> {
        self.dbfiles.as_ref()?.record_set(self, path, schema)
    }

    /// The chain's named files with sizes, for development and extraction; a file in no listfile
    /// (most of `texture.MPQ`) is readable by name but not listed. Unions every archive's
    /// `(listfile)`, as each names only its own files; sizes come from the winning archive.
    pub fn list(&self) -> Result<Vec<ChainEntry>> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for archive in &self.archives {
            let Ok(listfile) = archive.read_file("(listfile)") else {
                continue;
            };
            for raw in String::from_utf8_lossy(&listfile).split([';', '\r', '\n']) {
                let name = raw.trim();
                // Dedupe the way MPQ hashing compares names: any case, `/` and `\` alike.
                if name.is_empty() || !seen.insert(name.replace('/', "\\").to_ascii_lowercase()) {
                    continue;
                }
                if let Some(a) = self.resolve(name) {
                    // A tombstoned path is not a file in the composite.
                    if a.is_delete_marker(name) {
                        continue;
                    }
                    out.push(ChainEntry {
                        name: name.to_string(),
                        size: a.file_size(name).unwrap_or(0) as u64,
                    });
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn owned(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn patch_glob_matches_exactly_one_character_case_insensitively() {
        assert!(is_patch_glob_match("patch-2.MPQ"));
        assert!(is_patch_glob_match("patch-3.MPQ"));
        assert!(is_patch_glob_match("PATCH-A.mpq"));
        assert!(!is_patch_glob_match("patch-.MPQ"));
        assert!(!is_patch_glob_match("patch-10.MPQ"));
        assert!(!is_patch_glob_match("patch-33.MPQ"));
        assert!(!is_patch_glob_match("patch.MPQ"));
        assert!(!is_patch_glob_match("patch-2.MPQ.bak"));
        assert!(!is_patch_glob_match("mypatch-2.MPQ"));
    }

    #[test]
    fn mount_order_is_the_carved_law() {
        // base.MPQ is telemetry-only in the reference and never mounts.
        let dir = owned(&[
            "patch-2.MPQ",
            "backup.MPQ",
            "model.MPQ",
            "base.MPQ",
            "dbc.MPQ",
            "patch.MPQ",
            "eula.html",
            "patch-3.MPQ",
            "speech2.MPQ",
            "texture.MPQ",
        ]);
        assert_eq!(
            mount_order(&dir),
            owned(&[
                "dbc.MPQ",
                "texture.MPQ",
                "model.MPQ",
                "patch.MPQ",
                "patch-2.MPQ",
                "patch-3.MPQ",
                "speech2.MPQ",
            ])
        );
    }

    #[test]
    fn patch_sort_is_ascending_and_case_folded() {
        let dir = owned(&["patch-B.MPQ", "patch-3.MPQ", "patch-a.MPQ", "patch-2.MPQ"]);
        assert_eq!(
            mount_order(&dir),
            owned(&["patch-2.MPQ", "patch-3.MPQ", "patch-a.MPQ", "patch-B.MPQ"])
        );
    }

    #[test]
    fn base_archives_are_found_case_insensitively() {
        let dir = owned(&["DBC.mpq", "Model.MPQ", "PATCH.mpq"]);
        assert_eq!(
            mount_order(&dir),
            owned(&["DBC.mpq", "Model.MPQ", "PATCH.mpq"])
        );
    }

    /// A unique temp `Data` folder (pid and a counter), so parallel tests never collide.
    fn temp_data(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "benilla_chain_test_{}_{}_{}",
            std::process::id(),
            n,
            tag
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("make temp data dir");
        dir
    }

    /// Write `bytes` at `dir.join(rel)`, making the parent dirs.
    fn write_at(dir: &Path, rel: &str, bytes: &[u8]) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("make parent dirs");
        }
        std::fs::write(&p, bytes).expect("write loose file");
    }

    /// A `Data` folder whose patches are loose-file folders: they mount in the patch order, the
    /// later one wins, and names resolve case-insensitively with either separator.
    #[test]
    fn patch_folders_mount_in_the_patch_order_and_the_later_wins() {
        let data = temp_data("folders");
        write_at(&data, "patch-a.mpq/Interface/A.txt", b"a");
        write_at(&data, "patch-b.mpq/Interface/A.txt", b"b");
        write_at(&data, "patch-b.mpq/Interface/B.txt", b"b-only");
        // A folder under a non-mountable name is not an archive.
        write_at(&data, "notes/readme.txt", b"not an archive");

        let chain = Chain::open(&data).expect("open a chain of patch folders");
        assert!(
            chain.contains("Interface/A.txt"),
            "the shared path resolves"
        );
        assert_eq!(chain.read("Interface/A.txt").unwrap(), b"b", "patch-b wins");
        assert_eq!(chain.read("Interface/B.txt").unwrap(), b"b-only");
        // Case-folded, backslash spelling: the folder index compares as an MPQ hash does.
        assert_eq!(chain.read("interface\\a.txt").unwrap(), b"b");
        // The winning archive is the folder, at its path under `Data`.
        assert_eq!(
            chain.find_file_archive("Interface/B.txt"),
            Some(data.join("patch-b.mpq").as_path())
        );
        assert!(
            !chain.contains("readme.txt"),
            "a non-mountable folder is not an archive"
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    /// A file and a folder that differ only in case mount as one archive: the file, which is what
    /// the reference opens. The file is not a real MPQ, so the open refuses — proving it is the
    /// file the chain picked, not the folder that would have read cleanly.
    #[test]
    fn a_file_beats_a_case_variant_folder_under_the_same_name() {
        let data = temp_data("case_tie");
        write_at(&data, "patch-2.mpq/Interface/A.txt", b"folder");
        std::fs::write(data.join("patch-2.MPQ"), b"not an mpq").expect("write junk file patch");

        let err = Chain::open(&data)
            .err()
            .expect("the file patch is mounted, and it is not an MPQ");
        assert!(
            err.to_string().contains("patch-2.MPQ"),
            "the error must name the file that was picked: {err:#}"
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    /// A folder named for a patch with a `.patch` extension mounts where its `.mpq` twin would:
    /// `.mpq` and `.patch` folders interleave by stem in the patch order, the later one wins,
    /// and a `.patch` name that is not mountable is not an archive.
    #[test]
    fn patch_named_folders_mount_where_their_mpq_twins_would() {
        let data = temp_data("patch_ext");
        write_at(&data, "patch-a.mpq/Interface/A.txt", b"from mpq-a");
        write_at(&data, "patch-b.patch/Interface/A.txt", b"from patch-b");
        write_at(&data, "patch-b.patch/Interface/B.txt", b"from patch-b");
        write_at(&data, "patch-c.mpq/Interface/B.txt", b"from mpq-c");
        // A `.patch` folder under a non-mountable name is not an archive.
        write_at(&data, "notes.patch/readme.txt", b"not an archive");

        let chain = Chain::open(&data).expect("open a chain of .patch folders");
        assert_eq!(
            chain.read("Interface/A.txt").unwrap(),
            b"from patch-b",
            "the .patch folder beats the earlier .mpq one"
        );
        assert_eq!(
            chain.read("Interface/B.txt").unwrap(),
            b"from mpq-c",
            "the later .mpq folder beats the earlier .patch one"
        );
        assert_eq!(
            chain.find_file_archive("Interface/A.txt"),
            Some(data.join("patch-b.patch").as_path())
        );
        assert!(
            !chain.contains("readme.txt"),
            "a non-mountable .patch folder is not an archive"
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    /// A file and a `.patch` folder that mount to the same name are one archive: the file, which
    /// is what the reference opens. The file is not a real MPQ, so the open refuses — proving it
    /// is the file the chain picked, not the folder that would have read cleanly.
    #[test]
    fn a_file_beats_a_patch_named_folder_under_the_same_name() {
        let data = temp_data("patch_tie");
        write_at(&data, "patch-2.patch/Interface/A.txt", b"folder");
        std::fs::write(data.join("patch-2.MPQ"), b"not an mpq").expect("write junk file patch");

        let err = Chain::open(&data)
            .err()
            .expect("the file patch is mounted, and it is not an MPQ");
        assert!(
            err.to_string().contains("patch-2.MPQ"),
            "the error must name the file that was picked: {err:#}"
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    /// Two folders that mount to the same name — a `.mpq` name and a `.patch` one of the same
    /// stem — are one archive: the `.mpq` name, first in the deterministic sort.
    #[test]
    fn an_mpq_named_folder_beats_a_patch_named_twin_of_the_same_stem() {
        let data = temp_data("stem_tie");
        write_at(&data, "patch-2.patch/Interface/A.txt", b"from .patch");
        write_at(&data, "patch-2.MPQ/Interface/A.txt", b"from .mpq");

        let chain = Chain::open(&data).expect("one archive for the twin folders");
        assert_eq!(chain.read("Interface/A.txt").unwrap(), b"from .mpq");
        assert_eq!(
            chain.find_file_archive("Interface/A.txt"),
            Some(data.join("patch-2.MPQ").as_path())
        );
        let _ = std::fs::remove_dir_all(&data);
    }
}
