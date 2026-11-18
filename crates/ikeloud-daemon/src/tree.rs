//! The iCloud Drive tree kept from CloudKit's zone changes (decision 0025).
//!
//! The first call fetches all records of the zone, later calls only what
//! changed since the sync token; the tree turns them into the same listing
//! the Drive web service gives, etags included (a folder's is its record's
//! change tag, a file's `<structure>::<content>`).
//!
//! Strict on purpose: a listing missing a file would look like a deleted
//! file. A file without its content record, a name that is not text (Advanced
//! Data Protection), a name that is not a path component or a parent chain
//! that does not end make the whole tree unusable ([`Unusable`]): the caller
//! lists the folders instead. Items whose parents do not reach the root
//! (Recently Deleted) are not part of the Drive folder and are left out, as
//! the web service leaves them out.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ikeloud_api::{Item, Kind, ROOT, RecordKind, ZoneRecord};
use ikeloud_safety::{Etag, RelPath};
use ikeloud_sync::{DocId, RemoteFile};

use crate::remote::Listing;

/// A full fetch at least this often, so a lost change cannot linger.
const FULL_REFRESH: Duration = Duration::from_secs(24 * 60 * 60);

/// Deeper parent chains than this are taken for a loop.
const MAX_DEPTH: usize = 512;

/// One folder or file of the zone.
#[derive(Debug, Clone, Default)]
struct Node {
    folder: bool,
    /// From the structure record: the node exists in the tree.
    structure: Option<Structure>,
    /// From the content record (files): change tag, size, checksum.
    content: Option<(String, u64, Option<String>)>,
}

#[derive(Debug, Clone)]
struct Structure {
    parent: Option<String>,
    basename: Option<String>,
    extension: Option<String>,
    change_tag: String,
}

/// The tree cannot give a trustworthy listing; list the folders instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unusable(pub(crate) String);

/// A folder of the listing: ids and version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FolderIds {
    pub(crate) drivewsid: String,
    pub(crate) docwsid: String,
    pub(crate) etag: String,
}

/// What a built tree gives: the listing, the files and the folders.
pub(crate) struct Built {
    pub(crate) listing: Listing,
    pub(crate) items: Vec<(RelPath, Item)>,
    pub(crate) folders: Vec<(String, FolderIds)>,
}

/// The tree and where its changes stand.
#[derive(Debug, Default)]
pub(crate) struct CloudTree {
    nodes: HashMap<String, Node>,
    /// Sync token of the last applied page.
    pub(crate) token: Option<String>,
    /// When the fetch without a token began.
    full_at: Option<Instant>,
}

fn drive_id(kind: &str, id: &str) -> String {
    if id == "root" && kind == "FOLDER" {
        ROOT.to_string()
    } else {
        format!("{kind}::com.apple.CloudDocs::{id}")
    }
}

impl CloudTree {
    /// Whether the next fetch should start over without a token.
    pub(crate) fn wants_full(&self) -> bool {
        self.token.is_none() || self.full_at.is_none_or(|at| at.elapsed() > FULL_REFRESH)
    }

    /// Forgets everything; the next fetch is a full one.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Starts a full fetch.
    pub(crate) fn begin_full(&mut self) {
        self.reset();
        self.full_at = Some(Instant::now());
    }

    /// Applies a page of changed records. Idempotent: a page applied twice
    /// (after a failure before its token was kept) changes nothing more.
    pub(crate) fn apply(&mut self, records: Vec<ZoneRecord>) {
        for r in records {
            match r.kind {
                RecordKind::Folder | RecordKind::FileStructure => {
                    if r.deleted {
                        self.nodes.remove(&r.id);
                        continue;
                    }
                    let node = self.nodes.entry(r.id).or_default();
                    node.folder = r.kind == RecordKind::Folder;
                    node.structure = Some(Structure {
                        parent: r.parent,
                        basename: r.basename,
                        extension: r.extension,
                        change_tag: r.change_tag,
                    });
                }
                RecordKind::FileContent => {
                    if r.deleted {
                        if let Some(node) = self.nodes.get_mut(&r.id) {
                            node.content = None;
                        }
                        continue;
                    }
                    let node = self.nodes.entry(r.id).or_default();
                    node.content = Some((r.change_tag, r.size.unwrap_or(0), r.checksum));
                }
                RecordKind::Other => {}
            }
        }
        // A content record whose structure is gone belongs to nothing.
        self.nodes
            .retain(|_, node| node.structure.is_some() || node.content.is_some());
    }

    /// Full name of a node (with its extension), if it decodes.
    fn name(structure: &Structure) -> Option<String> {
        let base = structure.basename.as_ref()?;
        Some(match &structure.extension {
            Some(ext) => format!("{base}.{ext}"),
            None => base.clone(),
        })
    }

    /// The path of a node, `None` when its parents do not reach the root.
    fn path(
        &self,
        id: &str,
        cache: &mut HashMap<String, Option<String>>,
    ) -> Result<Option<String>, Unusable> {
        let mut chain: Vec<(&str, &Structure)> = Vec::new();
        let mut current = id;
        let resolved = loop {
            if current == "root" {
                break Some(String::new());
            }
            if let Some(known) = cache.get(current) {
                break known.clone();
            }
            let Some(structure) = self.nodes.get(current).and_then(|n| n.structure.as_ref()) else {
                break None;
            };
            chain.push((current, structure));
            if chain.len() > MAX_DEPTH {
                return Err(Unusable("the parents of an item go round in a loop".into()));
            }
            match &structure.parent {
                Some(parent) => current = parent,
                None => break None,
            }
        };
        let mut path = resolved;
        for (node_id, structure) in chain.into_iter().rev() {
            path = match path {
                None => None,
                Some(prefix) => {
                    let name = Self::name(structure)
                        .ok_or_else(|| Unusable("a name is not text (encrypted?)".into()))?;
                    if name.is_empty() || name.contains('/') {
                        return Err(Unusable(format!("unusable name {name:?}")));
                    }
                    Some(if prefix.is_empty() {
                        name
                    } else {
                        format!("{prefix}/{name}")
                    })
                }
            };
            cache.insert(node_id.to_string(), path.clone());
        }
        Ok(path)
    }

    /// The listing, the files and the folders of the tree.
    pub(crate) fn build(&self) -> Result<Built, Unusable> {
        let mut cache = HashMap::new();
        let mut built = Built {
            listing: Listing::default(),
            items: Vec::new(),
            folders: Vec::new(),
        };
        let parent_id = |s: &Structure| s.parent.clone().unwrap_or_else(|| "root".to_string());
        for (id, node) in &self.nodes {
            if id == "root" {
                continue;
            }
            let Some(structure) = &node.structure else {
                continue; // content of a file whose structure is gone
            };
            let Some(path) = self.path(id, &mut cache)? else {
                continue; // not under the root: Recently Deleted
            };
            let rel = RelPath::parse(path.as_str())
                .map_err(|_| Unusable(format!("unusable path {path:?}")))?;
            let parent_drivewsid = drive_id("FOLDER", &parent_id(structure));
            if node.folder {
                built.listing.folders.push(rel.clone());
                built.folders.push((
                    rel.as_str().to_string(),
                    FolderIds {
                        drivewsid: drive_id("FOLDER", id),
                        docwsid: id.clone(),
                        etag: structure.change_tag.clone(),
                    },
                ));
                continue;
            }
            let (content_tag, size, checksum) = node
                .content
                .clone()
                .ok_or_else(|| Unusable(format!("{path}: a file without its content")))?;
            let etag = format!("{}::{content_tag}", structure.change_tag);
            if let Some(checksum) = checksum {
                built.listing.checksums.insert(rel.clone(), checksum);
            }
            built.listing.files.push(RemoteFile {
                path: rel.clone(),
                doc_id: DocId::new(id.clone()),
                etag: Etag::new(etag.clone()),
                size,
            });
            built.items.push((
                rel.clone(),
                Item {
                    drivewsid: drive_id("FILE", id),
                    docwsid: id.clone(),
                    kind: Kind::File,
                    name: rel.name().to_string(),
                    size: Some(size),
                    etag,
                    parent: Some(parent_drivewsid),
                    modified: None,
                },
            ));
        }
        Ok(built)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: &str, parent: &str, name: &str, tag: &str) -> ZoneRecord {
        ZoneRecord {
            kind: RecordKind::Folder,
            id: id.into(),
            deleted: false,
            change_tag: tag.into(),
            parent: Some(parent.into()),
            basename: Some(name.into()),
            extension: None,
            size: None,
            basehash: None,
            checksum: None,
        }
    }

    fn file(id: &str, parent: &str, base: &str, ext: &str, tag: &str) -> ZoneRecord {
        ZoneRecord {
            kind: RecordKind::FileStructure,
            extension: (!ext.is_empty()).then(|| ext.to_string()),
            ..folder(id, parent, base, tag)
        }
    }

    fn content(id: &str, tag: &str, size: u64) -> ZoneRecord {
        ZoneRecord {
            kind: RecordKind::FileContent,
            id: id.into(),
            deleted: false,
            change_tag: tag.into(),
            parent: None,
            basename: None,
            extension: None,
            size: Some(size),
            basehash: None,
            checksum: None,
        }
    }

    fn deleted(kind: RecordKind, id: &str) -> ZoneRecord {
        ZoneRecord {
            kind,
            deleted: true,
            ..content(id, "", 0)
        }
    }

    fn files(tree: &CloudTree) -> Vec<(String, String, u64)> {
        let mut out: Vec<_> = tree
            .build()
            .expect("usable")
            .listing
            .files
            .into_iter()
            .map(|f| {
                (
                    f.path.as_str().to_string(),
                    f.etag.as_str().to_string(),
                    f.size,
                )
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn records_make_the_listing_with_web_service_etags() {
        let mut tree = CloudTree::default();
        // Content before structure, as pages may come.
        tree.apply(vec![
            content("P", "b", 390_363),
            folder("T", "root", "Test", "d"),
            file("P", "T", "photo-100057305", "jpg", "c"),
            file("R", "root", "README", "", "x"),
            content("R", "y", 5),
        ]);
        assert_eq!(
            files(&tree),
            vec![
                ("README".into(), "x::y".into(), 5),
                ("Test/photo-100057305.jpg".into(), "c::b".into(), 390_363),
            ]
        );
        let built = tree.build().unwrap();
        assert_eq!(built.listing.folders, vec![RelPath::parse("Test").unwrap()]);
        let (_, test) = &built.folders[0];
        assert_eq!(test.drivewsid, "FOLDER::com.apple.CloudDocs::T");
        assert_eq!(test.etag, "d");
        let photo = built
            .items
            .iter()
            .find(|(p, _)| p.name() == "photo-100057305.jpg")
            .unwrap();
        assert_eq!(
            photo.1.parent.as_deref(),
            Some("FOLDER::com.apple.CloudDocs::T")
        );
        let readme = built
            .items
            .iter()
            .find(|(p, _)| p.name() == "README")
            .unwrap();
        assert_eq!(readme.1.parent.as_deref(), Some(ROOT));
    }

    #[test]
    fn changes_rename_move_and_delete() {
        let mut tree = CloudTree::default();
        tree.apply(vec![
            folder("A", "root", "a", "1"),
            folder("B", "root", "b", "2"),
            file("F", "A", "f", "txt", "3"),
            content("F", "4", 1),
        ]);
        // Moved into b and renamed; a new version of the content.
        tree.apply(vec![file("F", "B", "g", "txt", "5"), content("F", "6", 2)]);
        assert_eq!(files(&tree), vec![("b/g.txt".into(), "5::6".into(), 2)]);
        // Folder renamed: the file's path follows.
        tree.apply(vec![folder("B", "root", "c", "7")]);
        assert_eq!(files(&tree)[0].0, "c/g.txt");
        // Deleted.
        tree.apply(vec![
            deleted(RecordKind::FileStructure, "F"),
            deleted(RecordKind::FileContent, "F"),
        ]);
        assert!(files(&tree).is_empty());
    }

    #[test]
    fn items_outside_the_root_are_left_out() {
        let mut tree = CloudTree::default();
        tree.apply(vec![
            folder("TR", "trash-root", "Smazané", "1"),
            file("F", "TR", "old", "txt", "2"),
            content("F", "3", 1),
        ]);
        assert!(files(&tree).is_empty());
    }

    #[test]
    fn doubtful_trees_are_unusable() {
        // A file without content.
        let mut tree = CloudTree::default();
        tree.apply(vec![file("F", "root", "f", "", "1")]);
        assert!(tree.build().is_err());
        // An encrypted name.
        let mut tree = CloudTree::default();
        let mut secret = folder("S", "root", "x", "1");
        secret.basename = None;
        tree.apply(vec![secret]);
        assert!(tree.build().is_err());
        // A name with a slash.
        let mut tree = CloudTree::default();
        tree.apply(vec![folder("S", "root", "a/b", "1")]);
        assert!(tree.build().is_err());
        // A loop.
        let mut tree = CloudTree::default();
        tree.apply(vec![folder("X", "Y", "x", "1"), folder("Y", "X", "y", "1")]);
        // Found, not walked endlessly.
        assert!(tree.build().is_err());
    }

    #[test]
    fn applying_a_page_twice_changes_nothing() {
        let page = vec![
            folder("A", "root", "a", "1"),
            file("F", "A", "f", "", "2"),
            content("F", "3", 9),
        ];
        let mut tree = CloudTree::default();
        tree.apply(page.clone());
        let once = files(&tree);
        tree.apply(page);
        assert_eq!(files(&tree), once);
    }
}
