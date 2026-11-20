//! Fixture: one WSS byte stream into a streamed VFS write.

wit_bindgen::generate!({ path: "wit", world: "stream-upload", generate_all });

use soil::streams::duplex;
use soil::time::time;
use soil::vfs_mut::vfs_mut;

struct Component;
export!(Component);

impl Guest for Component {
    fn run(args: Vec<String>) -> Result<(), String> {
        let path = args
            .first()
            .cloned()
            .unwrap_or_else(|| "uploads/wss.bin".to_string());
        let session = loop {
            match duplex::accept("wss").map_err(|error| format!("accept: {error:?}"))? {
                Some(session) => break session,
                None => {
                    time::tick(50).map_err(|error| format!("tick: {error:?}"))?;
                }
            }
        };
        let info = session.info();
        eprintln!("stream from {:?}", info.peer);
        let writer = vfs_mut::begin_write(
            &path,
            "application/octet-stream",
            None,
            None,
            None,
        )
        .map_err(|error| format!("begin-write: {error:?}"))?;
        let mut total = 0u64;
        loop {
            match session.read(64 * 1024).map_err(|error| format!("read: {error:?}"))? {
                Some(bytes) => {
                    total += bytes.len() as u64;
                    writer.write(&bytes).map_err(|error| format!("write: {error:?}"))?;
                }
                None => break,
            }
        }
        let result = writer.finish().map_err(|error| format!("finish: {error:?}"))?;
        session.close().map_err(|error| format!("close: {error:?}"))?;
        println!(
            "{{\"path\":\"{}\",\"bytes\":{},\"content_id\":\"{}\"}}",
            result.path, total, result.content_id
        );
        Ok(())
    }
}
