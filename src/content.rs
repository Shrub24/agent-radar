//! Immutable wire content and bounded MessagePack framing, independent of its producer.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::Arc;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const MAX_PAYLOAD: usize = 1024 * 1024;
pub const MAX_FRAME: usize = 4 * MAX_PAYLOAD;
const RETAINED_BYTES: usize = 16 * MAX_PAYLOAD;
const RETAINED_ENTRIES: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("content_too_large")]
    TooLarge,
    #[error("invalid MessagePack: {0}")]
    Codec(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("content hash mismatch")]
    HashMismatch,
}

struct Bounded {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::other("content_too_large"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Maps in a payload use ordered keys; sequences preserve their domain order.
pub fn encode<T: Serialize + ?Sized>(value: &T, limit: usize) -> Result<Vec<u8>, Error> {
    let mut out = Bounded {
        bytes: Vec::new(),
        limit,
        exceeded: false,
    };
    let result = value.serialize(&mut rmp_serde::Serializer::new(&mut out).with_struct_map());
    if out.exceeded {
        return Err(Error::TooLarge);
    }
    result.map_err(|error| Error::Codec(error.to_string()))?;
    Ok(out.bytes)
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    if bytes.len() > MAX_FRAME {
        return Err(Error::TooLarge);
    }
    let mut decoder = rmp_serde::Deserializer::new(io::Cursor::new(bytes));
    decoder.set_max_depth(64);
    let value = T::deserialize(&mut decoder).map_err(|e| Error::Codec(e.to_string()))?;
    if decoder.position() != bytes.len() as u64 {
        return Err(Error::Codec("trailing bytes".into()));
    }
    Ok(value)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Content {
    pub hash: String,
    #[serde(with = "shared_bytes")]
    pub bytes: Arc<[u8]>,
}

mod shared_bytes {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        bytes: &Arc<[u8]>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(bytes)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        decoder: D,
    ) -> Result<Arc<[u8]>, D::Error> {
        serde_bytes::ByteBuf::deserialize(decoder).map(|bytes| bytes.into_vec().into())
    }
}

impl Content {
    pub fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Self, Error> {
        let bytes: Arc<[u8]> = encode(value, MAX_PAYLOAD)?.into();
        Ok(Self {
            hash: digest(&bytes),
            bytes,
        })
    }

    pub fn read<T: DeserializeOwned>(&self) -> Result<T, Error> {
        if self.bytes.len() > MAX_PAYLOAD {
            return Err(Error::TooLarge);
        }
        if digest(&self.bytes) != self.hash {
            return Err(Error::HashMismatch);
        }
        decode(&self.bytes)
    }
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(bytes))
}

/// Current content is pinned; only superseded content counts toward history.
#[derive(Default)]
pub struct Store {
    current: Vec<Content>,
    retained: VecDeque<Content>,
    retained_bytes: usize,
}

impl Store {
    pub fn get(&self, hash: &str) -> Option<Content> {
        self.current
            .iter()
            .chain(&self.retained)
            .find(|c| c.hash == hash)
            .cloned()
    }

    pub fn pin(&mut self, candidates: Vec<Content>) -> Vec<Content> {
        let next: Vec<_> = candidates
            .into_iter()
            .map(|c| self.get(&c.hash).unwrap_or(c))
            .collect();
        for old in std::mem::take(&mut self.current) {
            if !next.iter().any(|c| c.hash == old.hash)
                && !self.retained.iter().any(|c| c.hash == old.hash)
            {
                self.retained_bytes += old.bytes.len();
                self.retained.push_back(old);
            }
        }
        self.retained.retain(|c| {
            if next.iter().any(|n| n.hash == c.hash) {
                self.retained_bytes -= c.bytes.len();
                false
            } else {
                true
            }
        });
        while self.retained_bytes > RETAINED_BYTES || self.retained.len() > RETAINED_ENTRIES {
            if let Some(old) = self.retained.pop_front() {
                self.retained_bytes -= old.bytes.len();
            }
        }
        self.current = next.clone();
        next
    }
}

pub fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<T, Error> {
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = frame_length(length)?;
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    decode(&bytes)
}

pub fn frame_length(prefix: [u8; 4]) -> Result<usize, Error> {
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(Error::TooLarge);
    }
    Ok(length)
}

pub fn frame<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, Error> {
    let body = encode(value, MAX_FRAME)?;
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

/// Scatter/gather frame: metadata is encoded per delivery, payload bytes are shared.
pub struct Segments(pub Vec<Arc<[u8]>>);

impl Segments {
    pub fn encoded<T: Serialize + ?Sized>(value: &T) -> Result<Self, Error> {
        Ok(Self(vec![frame(value)?.into()]))
    }
}

/// The portable envelope is `{metadata, content: [{hash, bytes}, ...]}`.
pub fn content_frame<T: Serialize>(metadata: &T, content: &[Content]) -> Result<Segments, Error> {
    let mut header = vec![0x82];
    header.extend(encode("metadata", MAX_FRAME)?);
    header.extend(encode(metadata, MAX_FRAME)?);
    header.extend(encode("content", MAX_FRAME)?);
    rmp::encode::write_array_len(
        &mut header,
        content.len().try_into().map_err(|_| Error::TooLarge)?,
    )
    .map_err(|e| Error::Codec(e.to_string()))?;
    let mut parts: Vec<Arc<[u8]>> = vec![header.into()];
    for payload in content {
        if payload.bytes.len() > MAX_PAYLOAD {
            return Err(Error::TooLarge);
        }
        let mut header = vec![0x82];
        header.extend(encode("hash", MAX_FRAME)?);
        header.extend(encode(&payload.hash, MAX_FRAME)?);
        header.extend(encode("bytes", MAX_FRAME)?);
        rmp::encode::write_bin_len(&mut header, payload.bytes.len() as u32)
            .map_err(|e| Error::Codec(e.to_string()))?;
        parts.push(header.into());
        parts.push(Arc::clone(&payload.bytes));
    }
    let size: usize = parts.iter().map(|p| p.len()).sum();
    if size > MAX_FRAME {
        return Err(Error::TooLarge);
    }
    parts.insert(0, Arc::from((size as u32).to_be_bytes()));
    Ok(Segments(parts))
}

pub fn write_frame<W: Write, T: Serialize + ?Sized>(
    writer: &mut W,
    value: &T,
) -> Result<(), Error> {
    writer.write_all(&frame(value)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// The same cases pi-radar's TypeScript codec reads, written by an independent encoder.
    #[test]
    fn independently_written_cases_match_this_codec_both_ways() {
        let file: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/msgpack/cases.json")).unwrap();
        for case in file["cases"].as_array().unwrap() {
            let name = &case["name"];
            let bytes = encode(&case["json"], MAX_PAYLOAD).unwrap();
            assert_eq!(hex(&bytes), case["hex"].as_str().unwrap(), "{name}");
            assert_eq!(
                decode::<serde_json::Value>(&bytes).unwrap(),
                case["json"],
                "{name}"
            );
        }
        for case in file["big"].as_array().unwrap() {
            let decimal = case["decimal"].as_str().unwrap();
            let value = decimal
                .parse::<u64>()
                .map(serde_json::Value::from)
                .or_else(|_| decimal.parse::<i64>().map(serde_json::Value::from))
                .unwrap();
            let bytes = encode(&value, MAX_PAYLOAD).unwrap();
            assert_eq!(hex(&bytes), case["hex"].as_str().unwrap(), "{decimal}");
        }
    }

    #[test]
    fn portable_content_has_stable_bytes_and_exact_integers() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Sample {
            count: u64,
        }
        let sample = Sample {
            count: 9_007_199_254_740_993,
        };
        let content = Content::encode(&sample).unwrap();
        assert_eq!(
            &*content.bytes,
            &[
                0x81, 0xa5, b'c', b'o', b'u', b'n', b't', 0xcf, 0, 0x20, 0, 0, 0, 0, 0, 1
            ]
        );
        assert_eq!(content.read::<Sample>().unwrap(), sample);
        let a: BTreeMap<_, _> = [("b", 2), ("a", 1)].into_iter().collect();
        let b: BTreeMap<_, _> = [("a", 1), ("b", 2)].into_iter().collect();
        assert_eq!(
            Content::encode(&a).unwrap().hash,
            Content::encode(&b).unwrap().hash
        );
    }

    #[test]
    fn retained_content_is_reused_and_history_is_bounded() {
        let mut store = Store::default();
        let a = Content::encode(&"A").unwrap();
        store.pin(vec![a.clone()]);
        store.pin(vec![Content::encode(&"B").unwrap()]);
        let restored = store.pin(vec![Content::encode(&"A").unwrap()]);
        assert!(Arc::ptr_eq(&a.bytes, &restored[0].bytes));
        for i in 0..40 {
            store.pin(vec![Content::encode(&i).unwrap()]);
        }
        assert!(store.get(&a.hash).is_none());
        assert!(store.retained.len() <= RETAINED_ENTRIES);
        assert!(store.retained_bytes <= RETAINED_BYTES);
    }

    #[test]
    fn segmented_frames_share_payload_bytes_and_decode_as_one_message() {
        #[derive(Deserialize)]
        struct Envelope {
            metadata: u64,
            content: Vec<Content>,
        }
        let payload = Content::encode(&"shared").unwrap();
        let segments =
            content_frame(&9_007_199_254_740_993u64, std::slice::from_ref(&payload)).unwrap();
        assert!(
            segments
                .0
                .iter()
                .any(|part| Arc::ptr_eq(part, &payload.bytes))
        );
        let bytes: Vec<u8> = segments.0.iter().flat_map(|p| p.iter().copied()).collect();
        let decoded: Envelope = read_frame(&mut &bytes[..]).unwrap();
        assert_eq!(decoded.metadata, 9_007_199_254_740_993);
        assert_eq!(decoded.content[0].hash, payload.hash);
        assert_eq!(decoded.content[0].read::<String>().unwrap(), "shared");
    }

    #[test]
    fn byte_retention_is_bounded_independently_of_entry_count() {
        let mut store = Store::default();
        let mut bytes = vec![0u8; MAX_PAYLOAD - 16];
        for i in 0..20 {
            bytes[0] = i;
            store.pin(vec![
                Content::encode(serde_bytes::Bytes::new(&bytes)).unwrap(),
            ]);
        }
        assert!(store.retained.len() < RETAINED_ENTRIES);
        assert!(store.retained_bytes <= RETAINED_BYTES);
    }

    #[test]
    fn bounded_encoding_and_frame_reception_reject_partial_or_excessive_data() {
        assert!(matches!(
            Content::encode(&vec![0u8; MAX_PAYLOAD + 1]),
            Err(Error::TooLarge)
        ));
        let prefix = ((MAX_FRAME + 1) as u32).to_be_bytes();
        assert!(matches!(
            read_frame::<_, String>(&mut &prefix[..]),
            Err(Error::TooLarge)
        ));
        let frame = frame(&"sample").unwrap();
        assert!(read_frame::<_, String>(&mut &frame[..frame.len() - 1]).is_err());
    }
}
