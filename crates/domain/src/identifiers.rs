//! Strongly typed, anonymous entity identifiers.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! entity_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Creates a random opaque identifier suitable for internal use.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wraps an existing UUID, for example when loading persisted data.
            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            /// Returns the underlying UUID.
            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self::from_uuid(value)
            }
        }
    };
}

entity_id!(SubjectId, "Anonymous identifier for a subject.");
entity_id!(VisitId, "Identifier for a subject visit.");
entity_id!(RecordingId, "Identifier for an EEG recording.");
entity_id!(ScaleResultId, "Identifier for a completed scale result.");
entity_id!(ModelResultId, "Identifier for a model inference result.");
entity_id!(AssessmentId, "Identifier for an integrated assessment.");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_type_safe_and_round_trip_as_json() {
        let id = SubjectId::new();
        let json = serde_json::to_string(&id).unwrap();
        let decoded: SubjectId = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, id);
        assert_eq!(id.to_string(), id.as_uuid().to_string());
    }
}
