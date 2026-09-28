//! Subject demographics kept intentionally free of direct identifiers.

use crate::{DomainError, Metadata, SubjectId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Sex recorded for research stratification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sex {
    Female,
    Male,
    Intersex,
    Other,
    Unknown,
}

/// An anonymized research subject.
///
/// Names, phone numbers, and national identifiers are deliberately absent. If
/// an installation must associate such data, it should do so in a separate,
/// access-controlled system rather than in the shared report model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subject {
    pub id: SubjectId,
    pub age: Option<u8>,
    pub sex: Sex,
    #[serde(default)]
    pub metadata: Metadata,
    pub created_at: DateTime<Utc>,
}

impl Subject {
    pub const MAX_AGE: u8 = 125;

    pub fn new(
        id: SubjectId,
        age: Option<u8>,
        sex: Sex,
        created_at: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        if let Some(age) = age
            && age > Self::MAX_AGE
        {
            return Err(DomainError::InvalidAge {
                max: Self::MAX_AGE,
                value: age,
            });
        }
        Ok(Self {
            id,
            age,
            sex,
            metadata: Metadata::new(),
            created_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_rejects_implausible_age() {
        let result = Subject::new(SubjectId::new(), Some(126), Sex::Unknown, Utc::now());
        assert_eq!(
            result.unwrap_err(),
            DomainError::InvalidAge {
                max: 125,
                value: 126
            }
        );
    }
}
