use super::*;
use crate::backlog::MoveMethod;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::acquisition) enum Profile {
    Move,
    Copy,
}

impl Profile {
    pub(in crate::acquisition) fn method(self) -> MoveMethod {
        match self {
            Self::Move => MoveMethod::UidMove,
            Self::Copy => MoveMethod::CopyUidExpunge,
        }
    }
    pub(in crate::acquisition) fn validate_capabilities(
        self,
        capabilities: &BTreeSet<String>,
    ) -> Result<(), Error> {
        let valid = capabilities.contains("UIDPLUS")
            && match self {
                Self::Move => capabilities.contains("MOVE"),
                Self::Copy => !capabilities.contains("MOVE") && capabilities.contains("CONDSTORE"),
            };
        if valid {
            Ok(())
        } else {
            Err(Error::Unsupported)
        }
    }
}

impl InstalledSource {
    pub(in crate::acquisition) fn profile(&self) -> Result<Profile, Error> {
        match self.authority.as_str() {
            "fixture-message-move/v1" | "live-message-move/v1" => Ok(Profile::Move),
            "fixture-message-copy/v1" => Ok(Profile::Copy),
            _ => Err(Error::Unsupported),
        }
    }
}

#[cfg(test)]
mod tests;
