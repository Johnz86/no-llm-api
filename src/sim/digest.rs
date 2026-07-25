//! A deterministic, in-crate digest.
//!
//! `DefaultHasher` is explicitly not stable across Rust releases or platforms,
//! so anything that picks a fixture or derives an id must not use it. FNV-1a is
//! specified byte for byte, which is what makes "the same request yields the same
//! response on any machine" testable.

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// A running FNV-1a 64-bit digest.
#[derive(Debug, Clone, Copy)]
pub struct Digest(u64);

impl Default for Digest {
    fn default() -> Self {
        Self(OFFSET_BASIS)
    }
}

impl Digest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write(&mut self, bytes: &[u8]) -> &mut Self {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(PRIME);
        }
        self
    }

    /// Writes a length-prefixed field so `["ab","c"]` and `["a","bc"]` differ.
    pub fn field(&mut self, bytes: &[u8]) -> &mut Self {
        self.write(&(bytes.len() as u64).to_le_bytes());
        self.write(bytes)
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

/// Digests a sequence of fields into one value.
pub fn digest_fields<'a, I>(fields: I) -> u64
where
    I: IntoIterator<Item = &'a str>,
{
    let mut digest = Digest::new();
    for field in fields {
        digest.field(field.as_bytes());
    }
    digest.finish()
}

/// Maps a digest onto `0..len` without modulo bias concerns that matter here.
pub fn pick(digest: u64, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (digest % len as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_published_fnv1a_vectors() {
        let mut digest = Digest::new();
        digest.write(b"");
        assert_eq!(digest.finish(), 0xcbf2_9ce4_8422_2325);

        let mut digest = Digest::new();
        digest.write(b"a");
        assert_eq!(digest.finish(), 0xaf63_dc4c_8601_ec8c);

        let mut digest = Digest::new();
        digest.write(b"foobar");
        assert_eq!(digest.finish(), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn field_framing_prevents_concatenation_collisions() {
        assert_ne!(digest_fields(["ab", "c"]), digest_fields(["a", "bc"]));
    }

    #[test]
    fn pick_is_stable_and_in_range() {
        let digest = digest_fields(["model", "hello"]);
        assert_eq!(pick(digest, 4), pick(digest, 4));
        assert!(pick(digest, 4) < 4);
        assert_eq!(pick(digest, 0), 0);
    }
}
