//! The three identities a managed execution is named by.
//!
//! A subject is a durable agent identity, a claim is one attachment of a subject
//! to a session, and a registration is one process incarnation. The three were
//! plain strings, and mixing them caused a real defect: a branch close resolved
//! its members by subject and then terminated by that id, so a close could signal
//! a different incarnation than the one it discovered.
//!
//! These types are distinct so that mistake cannot compile. There is no
//! `From<String>`, no `Deref<Target = str>` and no conversion between them; a
//! value of one kind is minted, parsed, or read from a record of that kind.
//!
//! Serialization is transparent, so records, the wire and the generated schemas
//! carry the same bare string they always did. Deserialization validates, which
//! turns an unreadable id in a stored record into the named startup refusal the
//! daemon already applies to a record it cannot decode.

use std::fmt;

use serde::{Deserialize, Serialize};

/// One canonical lowercase UUID, shared by the three identity types.
fn canonical(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return None;
    }
    for (index, &byte) in bytes.iter().enumerate() {
        match index {
            8 | 13 | 18 | 23 => {
                if byte != b'-' {
                    return None;
                }
            }
            _ if !byte.is_ascii_hexdigit() => return None,
            _ => {}
        }
    }
    Some(value.to_ascii_lowercase())
}

macro_rules! identity {
    ($name:ident, $what:literal) => {
        #[doc = concat!("A ", $what, ", as a validated canonical UUID.")]
        #[derive(
            Clone,
            Debug,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
            schemars::JsonSchema,
        )]
        #[serde(try_from = "String", into = "String")]
        #[schemars(with = "String")]
        pub struct $name(String);

        impl $name {
            /// Parses a canonical UUID, normalizing hex digits to lowercase.
            pub fn parse(value: &str) -> Option<Self> {
                canonical(value).map(Self)
            }

            /// Mints a fresh id. This is the only place one is created.
            pub fn random() -> Self {
                Self::parse(&crate::control_plane::store::random_uuid())
                    .expect("a freshly minted UUID is canonical")
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = String;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::parse(&value).ok_or_else(|| format!("{} is not a canonical UUID", $what))
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

identity!(SubjectId, "durable agent identity");
identity!(ClaimId, "subject/session attachment");
identity!(RegistrationId, "process incarnation");

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: &str = "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22";

    #[test]
    fn an_id_serializes_as_the_bare_string_it_always_was() {
        let id = SubjectId::parse(ONE).expect("canonical");
        assert_eq!(serde_json::to_value(&id).unwrap(), serde_json::json!(ONE));
        assert_eq!(
            serde_json::from_value::<SubjectId>(serde_json::json!(ONE)).unwrap(),
            id
        );
    }

    #[test]
    fn anything_but_a_canonical_uuid_is_refused() {
        for refused in [
            "",
            "not-an-id",
            "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b2",
            "8a1f5c30_6f4b_4c58_9c7b_2d0e1a9f4b22",
            "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b2z",
        ] {
            assert!(ClaimId::parse(refused).is_none(), "{refused} parsed");
            assert!(
                serde_json::from_value::<ClaimId>(serde_json::json!(refused)).is_err(),
                "{refused} decoded"
            );
        }
        // Hex is case-insensitive and stored lowercase.
        let upper = RegistrationId::parse(&ONE.to_uppercase()).expect("uppercase hex");
        assert_eq!(upper.as_str(), ONE);
    }

    #[test]
    fn a_minted_id_is_canonical_and_two_differ() {
        let first = SubjectId::random();
        let second = SubjectId::random();
        assert_eq!(SubjectId::parse(first.as_str()), Some(first.clone()));
        assert_ne!(first, second);
    }
}
