//! Compiled-in DSH dictionaries. This module performs no filesystem, profile or
//! network operations. Activation belongs to the guarded owned-runtime lifecycle.
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug)]
pub struct Resource {
    pub path: &'static str,
    pub bytes: &'static [u8],
}
/// Keep the sorted tree and licence in the executable; never load an optional
/// locale archive, a source checkout, or an npm-installed language supplement.
pub fn resources() -> &'static [Resource] {
    &[
        Resource {
            path: "LICENSE",
            bytes: include_bytes!("../locales/dsh/LICENSE"),
        },
        Resource {
            path: "lib/client.js",
            bytes: include_bytes!("../locales/dsh/lib/client.js"),
        },
        Resource {
            path: "lib/index.js",
            bytes: include_bytes!("../locales/dsh/lib/index.js"),
        },
        Resource {
            path: "overlay.yml",
            bytes: include_bytes!("../locales/dsh/overlay.yml"),
        },
        Resource {
            path: "package.json",
            bytes: include_bytes!("../locales/dsh/package.json"),
        },
    ]
}
/// Platform-independent content identity for immutable versioned resource caches.
/// Length-prefix both paths and content to prevent concatenation ambiguities.
pub fn bundle_id() -> String {
    let mut digest = Sha256::new();
    for file in resources() {
        digest.update((file.path.len() as u64).to_be_bytes());
        digest.update(file.path.as_bytes());
        digest.update((file.bytes.len() as u64).to_be_bytes());
        digest.update(file.bytes);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
