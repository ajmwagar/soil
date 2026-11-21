//! Put a file into a VFS root, or read one back.
//!
//! Documentation, transcripts and manifests are supposed to live in the VFS, but
//! nothing could write arbitrary bytes into one from a shell: `mush write` takes
//! its text as arguments, so a multi-line document cannot survive it. This is the
//! missing end of the pipe.
//!
//! ```sh
//! soil-vfs-put --root ~/.local/share/fungos/vfs/docs --path notes/x.md < x.md
//! soil-vfs-put --root ~/.local/share/fungos/vfs/docs --path notes/x.md --get
//! ```

use std::io::{Read, Write};

use anyhow::{Context, Result, bail};
use fungos_vfs::Vfs;

fn main() -> Result<()> {
    let mut root = None;
    let mut path = None;
    let mut get = false;
    let mut media_type = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next(),
            "--path" => path = args.next(),
            "--media-type" => media_type = args.next(),
            "--get" => get = true,
            other => bail!("unknown argument {other:?}"),
        }
    }
    let (Some(root), Some(path)) = (root, path) else {
        bail!("usage: soil-vfs-put --root DIR --path PATH [--media-type TYPE] [--get]");
    };
    let mut vfs = Vfs::open(&root).with_context(|| format!("open vfs {root}"))?;

    if get {
        let bytes = vfs.get(&path).with_context(|| format!("read {path}"))?;
        std::io::stdout().write_all(&bytes)?;
        return Ok(());
    }

    let mut bytes = Vec::new();
    std::io::stdin()
        .read_to_end(&mut bytes)
        .context("read stdin")?;
    let media_type = media_type.unwrap_or_else(|| media_type_for(&path).to_string());
    // A document is content: the same bytes at the same path replace, and the same
    // bytes at a new path are the same content id.
    let entry = vfs
        .put(&path, &bytes, &media_type, fungos_vfs::Expect::Any)
        .with_context(|| format!("write {path}"))?;
    println!(
        "{} v{} {} {} bytes",
        entry.path, entry.version, entry.content_id, entry.size
    );
    Ok(())
}

fn media_type_for(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("md") | Some("txt") | Some("rs") | Some("toml") | Some("yaml") | Some("yml")
        | Some("json") | Some("sh") | Some("wit") => "text/plain",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    }
}
