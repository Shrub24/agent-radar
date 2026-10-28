//! The TUI's typed producers over the shared content transport.

use serde::{Deserialize, Serialize};

use crate::control_plane::observation::{Snapshot, SourceState};
use crate::observation::ObservationState;

pub use crate::control_plane::public_contracts::ui_session::{
    AttachmentOpen, DuplexFrame, DuplexMetadata, DuplexOperation, DuplexReply, DuplexRequest, Head,
};

/// One publication as the session generates it, before it is framed for a
/// connection: the head plus whatever payloads that connection does not hold.
#[derive(Clone, Debug)]
pub struct Frame {
    pub head: Head,
    pub content: Vec<crate::content::Content>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Observation {
    pub collected_at: Option<String>,
    pub sources: Vec<SourceState>,
    pub tasks: Option<crate::control_plane::answer::TaskSnapshotData>,
    pub observation: ObservationState,
}

impl From<&Snapshot> for Observation {
    fn from(snapshot: &Snapshot) -> Self {
        Self {
            collected_at: snapshot.collected_at.clone(),
            sources: snapshot.sources.clone(),
            tasks: snapshot.tasks.clone(),
            observation: snapshot.observation.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Content;

    fn fixture(hex: &str) -> Vec<u8> {
        hex.trim()
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn duplex_reply_keeps_shared_content_and_exact_request_identity() {
        use crate::control_plane::public_contracts::ui_session as dto;
        let payload = Content::encode(&"content").unwrap();
        let metadata = dto::DuplexMetadata::Reply {
            id: 9_007_199_254_740_993,
            reply: dto::DuplexReply::Content {
                hash: payload.hash.clone(),
            },
        };
        let segments =
            crate::content::content_frame(&metadata, std::slice::from_ref(&payload)).unwrap();
        assert!(
            segments
                .0
                .iter()
                .any(|part| std::sync::Arc::ptr_eq(part, &payload.bytes))
        );
        let bytes: Vec<u8> = segments
            .0
            .iter()
            .flat_map(|part| part.iter().copied())
            .collect();
        let decoded: dto::DuplexFrame = crate::content::read_frame(&mut bytes.as_slice()).unwrap();
        assert!(matches!(
            decoded.metadata,
            dto::DuplexMetadata::Reply {
                id: 9_007_199_254_740_993,
                ..
            }
        ));
        assert_eq!(decoded.content[0].hash, payload.hash);
    }

    #[test]
    fn portable_payload_fixtures_pin_all_three_lane_layouts() {
        use crate::control_plane::public_contracts::ui_session as dto;
        let observation = fixture(include_str!(
            "../../tests/fixtures/tui/observation.msgpack.hex"
        ));
        let decoded: Observation = crate::content::decode(&observation).unwrap();
        assert!(decoded.observation.inventory().is_none());
        assert_eq!(
            &*Content::encode(&Observation::default()).unwrap().bytes,
            &observation
        );
        let session = fixture(include_str!("../../tests/fixtures/tui/session.msgpack.hex"));
        let decoded: dto::SessionState = crate::content::decode(&session).unwrap();
        assert_eq!(decoded.gate, 9_007_199_254_740_993);
        assert_eq!(
            &*Content::encode(&dto::SessionState {
                gate: 9_007_199_254_740_993,
                ..dto::SessionState::default()
            })
            .unwrap()
            .bytes,
            &session
        );
        let tree = fixture(include_str!("../../tests/fixtures/tui/tree.msgpack.hex"));
        let decoded: Vec<dto::TreeRow> = crate::content::decode(&tree).unwrap();
        assert_eq!(decoded[0].id, dto::UiRowId::Workspace { id: "w1".into() });
        assert!(decoded[0].has_children);
        assert_eq!(&*Content::encode(&decoded).unwrap().bytes, &tree);
    }

    #[test]
    fn observation_hash_excludes_delivery_revisions_but_preserves_collection_time() {
        let mut snapshot = Snapshot {
            epoch: "first".into(),
            revision: 1,
            collected_at: Some("2026-10-27T10:00:00Z".into()),
            sources: Vec::new(),
            tasks: None,
            observation: ObservationState::default(),
        };
        let first = Content::encode(&Observation::from(&snapshot)).unwrap();
        snapshot.epoch = "restart".into();
        snapshot.revision = 900;
        assert_eq!(
            first.hash,
            Content::encode(&Observation::from(&snapshot)).unwrap().hash
        );
        snapshot.collected_at = Some("2026-10-27T10:00:01Z".into());
        assert_ne!(
            first.hash,
            Content::encode(&Observation::from(&snapshot)).unwrap().hash
        );
    }
}
