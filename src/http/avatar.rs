use std::path::{Path, PathBuf};

use crate::store::{Store, StoreError};

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
    #[error("store: {0}")]
    Store(#[from] StoreError),
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

pub async fn read(store: &Store, root: &Path, identity: &str) -> Result<Avatar, AvatarError> {
    let path = cache_path(root, identity)?;
    if let Ok(bytes) = tokio::fs::read(&path).await {
        return Ok(Avatar {
            content_type: content_type(&bytes),
            bytes,
            cacheable: true,
        });
    }

    let Some(url) = store.avatar_url_for_identity(identity).await? else {
        return Ok(generated(identity));
    };
    let Some(bytes) = fetch(&url).await else {
        return Ok(generated(identity));
    };

    tokio::fs::create_dir_all(root).await?;
    tokio::fs::write(&path, &bytes).await?;

    Ok(Avatar {
        content_type: content_type(&bytes),
        bytes,
        cacheable: true,
    })
}

/// A forge that is unreachable, slow, or no longer hosting the picture must not fail the
/// request: a generated blob is a better answer than a broken image.
async fn fetch(url: &str) -> Option<Vec<u8>> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(2))
        .timeout(std::time::Duration::from_secs(FETCH_SECONDS))
        .user_agent(concat!("error.menu/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let response = client.get(url).send().await.ok()?.error_for_status().ok()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_AVATAR_BYTES as u64)
    {
        return None;
    }

    let bytes = response.bytes().await.ok()?;

    (bytes.len() <= MAX_AVATAR_BYTES).then(|| bytes.to_vec())
}

fn content_type(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xff, 0xd8, 0xff, ..] => "image/jpeg",
        [b'G', b'I', b'F', ..] => "image/gif",
        [b'R', b'I', b'F', b'F', ..] => "image/webp",
        _ => "image/svg+xml",
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
}
