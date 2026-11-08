use std::hash::{DefaultHasher, Hash, Hasher};

/// A repeatable UUID for a named presentation fixture.
pub fn registration_id(name: &str) -> String {
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    format!(
        "00000000-0000-4000-{:04x}-{:012x}",
        0x8000 | (hasher.finish() >> 48) as u16 & 0x3fff,
        hasher.finish() & 0xffffffffffff
    )
}
