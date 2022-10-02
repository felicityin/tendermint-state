use bytes::Bytes;
use derive_more::Display;
use serde::{Deserialize, Serialize};

use crate::error::ConsensusError;
use crate::smr::smr_types::{Step, TriggerType};

/// Address type.
pub type Address = Bytes;
/// Hash type.
pub type Hash = Bytes;
/// Signature type.
pub type Signature = Bytes;

pub type ConsensusResult<T> = std::result::Result<T, ConsensusError>;

pub const INIT_HEIGHT: u64 = 0;
pub const INIT_ROUND: u64 = 0;

/// Vote or QC types. Prevote and precommit QC will promise the rightness and the final consistency
/// of overlord consensus protocol.
#[derive(Serialize, Deserialize, Clone, Debug, Display, PartialEq, Eq, Hash)]
pub enum VoteType {
    /// Prevote vote or QC.
    #[display(fmt = "Prevote")]
    Prevote,
    /// Precommit Vote or QC.
    #[display(fmt = "Precommit")]
    Precommit,
}

impl From<VoteType> for u8 {
    fn from(v: VoteType) -> u8 {
        match v {
            VoteType::Prevote => 1,
            VoteType::Precommit => 2,
        }
    }
}

impl From<VoteType> for TriggerType {
    fn from(v: VoteType) -> TriggerType {
        match v {
            VoteType::Prevote => TriggerType::PrevoteQC,
            VoteType::Precommit => TriggerType::PrecommitQC,
        }
    }
}

impl From<VoteType> for Step {
    fn from(v: VoteType) -> Step {
        match v {
            VoteType::Prevote => Step::Prevote,
            VoteType::Precommit => Step::Precommit,
        }
    }
}

impl TryFrom<u8> for VoteType {
    type Error = ConsensusError;

    fn try_from(s: u8) -> Result<Self, Self::Error> {
        match s {
            1 => Ok(VoteType::Prevote),
            2 => Ok(VoteType::Precommit),
            _ => Err(ConsensusError::Other("".to_string())),
        }
    }
}

/// The reason of overlord view change.
#[derive(Serialize, Deserialize, Clone, Debug, Display)]
pub enum ViewChangeReason {
    ///
    #[display(fmt = "Do not receive proposal from network")]
    NoProposalFromNetwork,

    ///
    #[display(fmt = "Do not receive Prevote QC from network")]
    NoPrevoteQCFromNetwork,

    ///
    #[display(fmt = "Do not receive precommit QC from network")]
    NoPrecommitQCFromNetwork,

    ///
    #[display(fmt = "Check the block not pass")]
    CheckBlockNotPass,

    ///
    #[display(fmt = "Update from a higher round prevote QC from {} to {}", _0, _1)]
    UpdateFromHigherPrevoteQC(u64, u64),

    ///
    #[display(fmt = "Update from a higher round precommit QC from {} to {}", _0, _1)]
    UpdateFromHigherPrecommitQC(u64, u64),

    ///
    #[display(fmt = "Update from a higher round choke QC from {} to {}", _0, _1)]
    UpdateFromHigherChokeQC(u64, u64),

    ///
    #[display(fmt = "{:?} votes count is below threshold", _0)]
    LeaderReceivedVoteBelowThreshold(VoteType),

    ///
    #[display(fmt = "other reasons")]
    Others,
}

/// The setting of the timeout interval of each step.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
pub struct DurationConfig {
    /// The proportion of propose timeout to the height interval.
    pub propose_ratio: u64,
    /// The proportion of prevote timeout to the height interval.
    pub prevote_ratio: u64,
    /// The proportion of precommit timeout to the height interval.
    pub precommit_ratio: u64,
    /// The proportion of retry choke message timeout to the height interval.
    pub brake_ratio: u64,
}

impl DurationConfig {
    /// Create a consensus timeout configuration.
    pub fn new(
        propose_ratio: u64,
        prevote_ratio: u64,
        precommit_ratio: u64,
        brake_ratio: u64,
    ) -> Self {
        DurationConfig {
            propose_ratio,
            prevote_ratio,
            precommit_ratio,
            brake_ratio,
        }
    }
}
