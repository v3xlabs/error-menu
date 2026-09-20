use std::path::{Path, PathBuf};
use std::sync::Arc;

use poem::http::StatusCode;
use tokio::io::AsyncReadExt;

use crate::app::AppState;
use crate::database::DatabaseError;
use crate::project::person::Person;

const MAX_AVATAR_BYTES: usize = 512 * 1024;
const FETCH_SECONDS: u64 = 10;

/// A person's picture, ready to serve. A generated blob is produced locally and is never
/// written to the cache, so a forge avatar that arrives later replaces it.
pub struct Avatar {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub cacheable: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum AvatarError {
    #[error("an identity is 32 hexadecimal characters")]
    Identity,
    #[error("database: {0}")]
    Database(#[from] DatabaseError),
    #[error("filesystem: {0}")]
    Io(#[from] std::io::Error),
}

/// The identity comes from a URL, so it is checked before it reaches the filesystem. A
/// blake3 prefix is the only shape that can name a cache file.
pub fn cache_path(root: &Path, identity: &str) -> Result<PathBuf, AvatarError> {
    let valid = identity.len() == 32 && identity.bytes().all(|byte| byte.is_ascii_hexdigit());

    valid
        .then(|| root.join(identity))
        .ok_or(AvatarError::Identity)
}

pub async fn read(state: &AppState, identity: &str) -> Result<Avatar, AvatarError> {
    let path = cache_path(&state.avatars, identity)?;
    if let Ok(file) = tokio::fs::File::open(&path).await {
        let mut bytes = Vec::new();
        if file
            .take((MAX_AVATAR_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .is_ok()
            && bytes.len() <= MAX_AVATAR_BYTES
            && let Some(content_type) = content_type(&bytes)
        {
            return Ok(Avatar {
                content_type,
                bytes,
                cacheable: true,
            });
        }
    }

    let Some(url) = Person::avatar_url(&state.database, identity).await? else {
        return Ok(generated(identity));
    };
    let Some((bytes, content_type)) = fetch(&url).await else {
        return Ok(generated(identity));
    };

    tokio::fs::create_dir_all(&state.avatars).await?;
    tokio::fs::write(&path, &bytes).await?;

    Ok(Avatar {
        content_type,
        bytes,
        cacheable: true,
    })
}

#[poem::handler]
pub async fn serve(
    poem::web::Path(identity): poem::web::Path<String>,
    poem::web::Data(state): poem::web::Data<&Arc<AppState>>,
) -> poem::Response {
    match read(state, &identity).await {
        Ok(picture) => poem::Response::builder()
            .content_type(picture.content_type)
            .header(
                "cache-control",
                if picture.cacheable {
                    "public, max-age=86400"
                } else {
                    "no-store"
                },
            )
            .body(picture.bytes),
        Err(AvatarError::Identity) => poem::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .finish(),
        Err(error) => poem::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(error.to_string()),
    }
}

/// A forge that is unreachable, slow, or no longer hosting the picture must not fail the
/// request: a generated blob is a better answer than a broken image.
async fn fetch(raw: &str) -> Option<(Vec<u8>, &'static str)> {
    let url = url::Url::parse(raw).ok()?;
    crate::outbound::validate_url(&url).ok()?;
    let client = crate::outbound::client().ok()?;
    let mut response = client
        .get(url)
        .timeout(std::time::Duration::from_secs(FETCH_SECONDS))
        .send()
        .await
        .ok()?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > MAX_AVATAR_BYTES as u64)
    {
        return None;
    }

    let mut bytes = Vec::new();
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => return None,
        };
        append_chunk(&mut bytes, &chunk)?;
    }
    let content_type = content_type(&bytes)?;
    Some((bytes, content_type))
}

fn append_chunk(bytes: &mut Vec<u8>, chunk: &[u8]) -> Option<()> {
    if chunk.len() > MAX_AVATAR_BYTES.saturating_sub(bytes.len()) {
        return None;
    }
    bytes.extend_from_slice(chunk);
    Some(())
}

fn content_type(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, ..] => Some("image/png"),
        [0xff, 0xd8, 0xff, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', b'7' | b'9', b'a', ..] => Some("image/gif"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("image/webp"),
        _ => None,
    }
}

/// A deterministic blob, so that the same person keeps the same picture on every machine
/// and across restarts without any request leaving this one.
fn generated(identity: &str) -> Avatar {
    let seed = u32::from_str_radix(&identity[..6], 16).unwrap_or(0);
    let hue = seed % 360;
    let partner = (hue + 140) % 360;
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" role="img"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl({hue} 70% 62%)"/><stop offset="1" stop-color="hsl({partner} 68% 48%)"/></linearGradient></defs><rect width="64" height="64" rx="32" fill="url(#g)"/><circle cx="32" cy="25" r="11" fill="rgba(255,255,255,0.85)"/><path d="M12 62c0-12 9-20 20-20s20 8 20 20z" fill="rgba(255,255,255,0.85)"/></svg>"##
    );

    Avatar {
        bytes: svg.into_bytes(),
        content_type: "image/svg+xml",
        cacheable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_cannot_walk_out_of_the_cache_directory() {
        assert!(matches!(
            cache_path(Path::new(".tmp/avatars"), "../../etc/passwd"),
            Err(AvatarError::Identity)
        ));
    }

    #[test]
    fn a_blake3_prefix_names_a_cache_file() {
        let path = cache_path(Path::new(".tmp/avatars"), &"a1b2c3d4".repeat(4))
            .expect("a hexadecimal identity is accepted");

        assert!(path.starts_with(".tmp/avatars"));
    }

    #[test]
    fn a_person_without_a_forge_avatar_still_gets_a_picture() {
        let avatar = generated(&"f0e1d2c3".repeat(4));

        assert_eq!(avatar.content_type, "image/svg+xml");
        assert!(!avatar.cacheable);
        assert!(!avatar.bytes.is_empty());
    }

    #[test]
    fn the_same_person_always_gets_the_same_picture() {
        let identity = "9".repeat(32);

        assert_eq!(generated(&identity).bytes, generated(&identity).bytes);
    }

    #[test]
    fn avatar_chunks_stop_before_exceeding_the_limit() {
        let mut bytes = vec![0; MAX_AVATAR_BYTES - 1];
        assert_eq!(append_chunk(&mut bytes, &[1]), Some(()));
        assert_eq!(append_chunk(&mut bytes, &[2]), None);
        assert_eq!(bytes.len(), MAX_AVATAR_BYTES);
        assert_eq!(bytes[MAX_AVATAR_BYTES - 1], 1);

        let mut bytes = vec![0; MAX_AVATAR_BYTES - 2];
        assert_eq!(append_chunk(&mut bytes, &[1, 2, 3]), None);
        assert_eq!(bytes.len(), MAX_AVATAR_BYTES - 2);
    }

    #[test]
    fn remote_content_must_be_a_supported_raster_image() {
        assert_eq!(
            content_type(b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
            None
        );
        assert_eq!(
            content_type(b"<!doctype html><script>alert(1)</script>"),
            None
        );
        assert_eq!(content_type(b"RIFF\x00\x00\x00\x00WAVE"), None);
        assert_eq!(content_type(b"GIF<script>"), None);
        assert_eq!(content_type(b"\x89PNG\r\n\x1a\n"), Some("image/png"));
        assert_eq!(content_type(b"GIF89a"), Some("image/gif"));
        assert_eq!(
            content_type(b"RIFF\x00\x00\x00\x00WEBP"),
            Some("image/webp")
        );
    }
}
