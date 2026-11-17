//! `soil:vfs-cid`: a photo's record names bytes, and this is the lookup that
//! turns that name back into places inside the workspace.

use soil::vfs_host::{NodeRole, VfsHost, cid, vfs_mut};

fn host() -> (tempfile::TempDir, VfsHost, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let host = VfsHost::open_with_role_and_store(root.path(), NodeRole::Full, Some(store.path()))
        .expect("a vfs host");
    (root, host, store)
}

fn write(host: &mut VfsHost, path: &str, bytes: &[u8]) -> String {
    let result = <VfsHost as vfs_mut::Host>::write(
        host,
        path.to_string(),
        bytes.to_vec(),
        "image/jpeg".to_string(),
        None,
        None,
    )
    .expect("a write");
    result.content_id
}

#[test]
fn find_answers_every_path_holding_the_bytes() {
    let (_root, mut host, _store) = host();
    let bytes = b"the same bytes under two names";
    let first = write(&mut host, "photos/a.jpg", bytes);
    let second = write(&mut host, "photos/nested/b.jpg", bytes);
    assert_eq!(first, second, "identity is the bytes, not the name");
    write(&mut host, "photos/other.jpg", b"different bytes");

    let page = <VfsHost as cid::Host>::find(&mut host, first.clone(), 0, 8).expect("a page");
    let paths: Vec<&str> = page
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    assert_eq!(paths, vec!["photos/a.jpg", "photos/nested/b.jpg"]);
    assert_eq!(page.entries[0].content_id, first);
    assert_eq!(page.entries[0].size, bytes.len() as u64);
    assert!(page.next.is_none());
}

#[test]
fn find_pages_in_path_order() {
    let (_root, mut host, _store) = host();
    let bytes = b"shared";
    let id = write(&mut host, "c.jpg", bytes);
    write(&mut host, "a.jpg", bytes);
    write(&mut host, "b.jpg", bytes);

    let first = <VfsHost as cid::Host>::find(&mut host, id.clone(), 0, 2).expect("a page");
    assert_eq!(first.entries.len(), 2);
    assert_eq!(first.entries[0].path, "a.jpg");
    assert_eq!(first.entries[1].path, "b.jpg");
    assert_eq!(first.next, Some(2));

    let second = <VfsHost as cid::Host>::find(&mut host, id, 2, 2).expect("a page");
    assert_eq!(second.entries.len(), 1);
    assert_eq!(second.entries[0].path, "c.jpg");
    assert!(second.next.is_none());
}

#[test]
fn an_unknown_content_id_is_an_empty_answer_and_a_malformed_one_is_an_error() {
    let (_root, mut host, _store) = host();
    write(&mut host, "a.jpg", b"present");

    let unknown = format!("blake3:{}", "00".repeat(32));
    let page = <VfsHost as cid::Host>::find(&mut host, unknown, 0, 8).expect("a page");
    assert!(page.entries.is_empty(), "absent bytes are not an error");

    for bad in ["", "sha256:00", "blake3:zz", "blake3:abcd"] {
        let error = <VfsHost as cid::Host>::find(&mut host, bad.to_string(), 0, 8)
            .expect_err("a malformed id is refused");
        assert_eq!(error, cid::Error::Invalid, "{bad:?}");
    }

    let zero = <VfsHost as cid::Host>::find(&mut host, format!("blake3:{}", "11".repeat(32)), 0, 0)
        .expect_err("a zero page is refused");
    assert_eq!(zero, cid::Error::OverBudget);
}

/// The whole path a viewer walks: an asset record names `blake3:…`, `find` says where it
/// lives, and `read` hands back the picture. If any link in that chain drifts, a photo grid
/// draws placeholder tiles and says nothing — which is exactly what this pins.
#[test]
fn the_path_find_answers_reads_back_the_bytes_the_record_named() {
    use soil::vfs_host::vfs;

    let (_root, mut host, _store) = host();
    let picture = b"a preview, whole, in this workspace";
    let id = write(&mut host, "photos/one.jpg", picture);

    let page = <VfsHost as cid::Host>::find(&mut host, id, 0, 8).expect("a page");
    let path = page
        .entries
        .first()
        .expect("the bytes are somewhere")
        .path
        .clone();
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk =
            <VfsHost as vfs::Host>::read(&mut host, path.clone(), offset, 4096).expect("a chunk");
        bytes.extend_from_slice(&chunk.data);
        offset += chunk.data.len() as u64;
        if chunk.eof {
            break;
        }
    }
    assert_eq!(bytes, picture);
}
