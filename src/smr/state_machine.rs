use derive_more::Display;
use futures::channel::mpsc::{unbounded, UnboundedSender};
use hummer::coding::hex_encode;

use crate::smr::smr_types::{
    FromWhere, Lock, SMREvent, SMRStatus, SMRTrigger, Step, TriggerSource, TriggerType,
};
use crate::{error::ConsensusError, smr::Event, types::Hash};
use crate::types::{ConsensusResult, INIT_HEIGHT, INIT_ROUND};

#[derive(Debug, Display)]
#[rustfmt::skip]
#[display(fmt = "State machine height {}, round {}, step {:?}", height, round, step)]
pub struct StateMachine {
    height:        u64,
    round:         u64,
    step:          Step,
    block_hash:    Hash,
    lock:          Option<Lock>,

    event:   (UnboundedSender<SMREvent>, UnboundedSender<SMREvent>),
}

impl StateMachine {
    /// Create a new state machine.
    pub fn new() -> (Self, Event, Event) {
        let (tx_state, rx_state) = unbounded();
        let (tx_timer, rx_timer) = unbounded();

        let state_machine = StateMachine {
            height: INIT_HEIGHT,
            round: INIT_ROUND,
            step: Step::default(),
            block_hash: Hash::new(),
            lock: None,
            event: (tx_state, tx_timer),
        };

        (state_machine, Event::new(rx_state), Event::new(rx_timer))
    }

    pub fn process(&mut self, msg: SMRTrigger) -> ConsensusResult<()> {
        let trigger_type = msg.trigger_type.clone();
        let res = match trigger_type {
            TriggerType::NewHeight(status) => {
                self.handle_new_height(status, msg.source)
            }
            TriggerType::Proposal => self.handle_proposal(
                msg.hash,
                msg.round,
                msg.lock_round,
                msg.source,
                msg.height,
            ),
            TriggerType::PrevoteQC => {
                self.handle_prevote(msg.hash, msg.round, msg.source, msg.height)
            }
            TriggerType::PrecommitQC => {
                self.handle_precommit(msg.hash, msg.round, msg.source, msg.height)
            }
            TriggerType::ContinueRound => {
                assert!(msg.source == TriggerSource::State);
                self.handle_continue_round(msg.height, msg.round)
            }
        };
        return res;
    }

    /// Handle a new height trigger. If new height is higher than current, goto new height and
    /// throw a new round info event.
    fn handle_new_height(
        &mut self,
        status: SMRStatus,
        source: TriggerSource,
    ) -> ConsensusResult<()> {
        log::debug!("Tendermint: SMR triggered by new height {}", status.height);

        let height = status.height;
        if source != TriggerSource::State {
            return Err(ConsensusError::Other(
                "Rich status source error".to_string(),
            ));
        } else if height <= self.height {
            return Err(ConsensusError::Other("Delayed status".to_string()));
        }

        self.goto_new_height(height);
        self.send_event(SMREvent::NewRoundInfo {
            height: self.height,
            round: INIT_ROUND,
            lock_round: None,
            lock_proposal: None,
            new_interval: status.new_interval,
            new_config: status.new_config,
            from_where: FromWhere::PrecommitQC(u64::max_value()),
        })?;
        self.goto_step(Step::Propose);
        Ok(())
    }

    /// Handle a proposal trigger. Only if self step is propose, the proposal is valid.
    /// If proposal hash is empty, prevote to an empty hash. If the lock round is some, and the lock
    /// round is higher than self lock round, remove PoLC. Finally throw prevote vote event. It is
    /// impossible that the proposal hash is empty with the lock round is some.
    fn handle_proposal(
        &mut self,
        proposal_hash: Hash,
        round: u64,
        lock_round: Option<u64>,
        source: TriggerSource,
        height: u64,
    ) -> ConsensusResult<()> {
        if self.height != height || self.round != round {
            return Ok(());
        }

        if self.step > Step::Propose {
            return Ok(());
        }

        log::debug!(
            "Tendermint: SMR triggered by a proposal hash {:?}, from {:?}, height {}, round {}",
            hex_encode(proposal_hash.clone()),
            source,
            self.height,
            self.round
        );

        // If the proposal trigger is from timer, goto prevote step directly.
        if source == TriggerSource::Timer {
            // This event is for timer to set a prevote timer.
            let (round, hash) = if let Some(lock) = &self.lock {
                (Some(lock.round), lock.hash.clone())
            } else {
                (None, Hash::new())
            };

            self.send_event(SMREvent::PrevoteVote {
                height: self.height,
                round: self.round,
                block_hash: hash,
                lock_round: round,
            })?;
            self.goto_step(Step::Prevote);
            return Ok(());
        } else if proposal_hash.is_empty() {
            return Err(ConsensusError::ProposalErr("Empty proposal".to_string()));
        }

        // update PoLC
        self.check()?;
        if let Some(lock_round) = lock_round {
            if let Some(lock) = self.lock.clone() {
                log::debug!("Tendermint: SMR handle proposal with a lock");

                if lock_round > lock.round {
                    self.remove_polc();
                    self.set_proposal(proposal_hash);
                } else if lock_round == lock.round && proposal_hash != self.block_hash {
                    return Err(ConsensusError::CorrectnessErr("Fork".to_string()));
                }
            } else {
                self.set_proposal(proposal_hash);
            }
        } else if self.lock.is_none() {
            self.set_proposal(proposal_hash);
        }

        let round = self.lock.as_ref().map(|lock| lock.round);

        self.send_event(SMREvent::PrevoteVote {
            height: self.height,
            round: self.round,
            block_hash: self.block_hash.clone(),
            lock_round: round,
        })?;
        self.goto_step(Step::Prevote);
        Ok(())
    }

    /// Handle a prevote quorum certificate trigger. Only if self step is prevote, the prevote QC is
    /// valid.  
    /// The prevote round must be some. If the vote round is higher than self lock round, update
    /// PoLC. Finally throw precommit vote event.
    fn handle_prevote(
        &mut self,
        prevote_hash: Hash,
        prevote_round: u64,
        source: TriggerSource,
        height: u64,
    ) -> ConsensusResult<()> {
        if self.height != height {
            return Ok(());
        }

        if prevote_round == self.round && self.step > Step::Prevote {
            return Ok(());
        }

        log::debug!(
            "Tendermint: SMR triggered by prevote QC hash {:?} qc round {} from {:?}, height {}, round {}",
            hex_encode(prevote_hash.clone()),
            prevote_round,
            source,
            self.height,
            self.round
        );

        if source == TriggerSource::Timer {
            if prevote_round != self.round {
                return Ok(());
            }

            // This event is for timer to set a precommit timer.
            let round = if let Some(lock) = &self.lock {
                Some(lock.round)
            } else {
                self.block_hash = Hash::new();
                None
            };

            self.send_event(SMREvent::PrecommitVote {
                height: self.height,
                round: self.round,
                block_hash: Hash::new(),
                lock_round: round,
            })?;
            self.goto_step(Step::Precommit);
            return Ok(());
        }

        // A prevote QC from timer which means prevote timeout can not lead to unlock. Therefore,
        // only prevote QCs from state will update the PoLC. If the prevote QC is from timer, goto
        // precommit step directly.
        self.check()?;

        if prevote_round < self.round {
            return Ok(());
        }

        self.update_polc(prevote_hash, prevote_round);

        if prevote_round > self.round {
            let (lock_round, lock_proposal) = self
                .lock
                .clone()
                .map_or_else(|| (None, None), |lock| (Some(lock.round), Some(lock.hash)));

            self.round = prevote_round;
            self.send_event(SMREvent::NewRoundInfo {
                height: self.height,
                round: self.round + 1,
                lock_round,
                lock_proposal,
                new_interval: None,
                new_config: None,
                from_where: FromWhere::PrevoteQC(prevote_round),
            })?;
            self.goto_next_round();
        }

        // throw precommit vote event
        let round = self.lock.as_ref().map(|lock| lock.round);
        self.send_event(SMREvent::PrecommitVote {
            height: self.height,
            round: self.round,
            block_hash: self.block_hash.clone(),
            lock_round: round,
        })?;
        self.goto_step(Step::Precommit);
        Ok(())
    }

    /// Handle a precommit quorum certificate trigger. Only if self step is precommit, the precommit
    /// QC is valid.
    /// The precommit round must be some. If its hash is empty, throw new round event and goto next
    /// round. Otherwise, throw commit event.
    fn handle_precommit(
        &mut self,
        precommit_hash: Hash,
        precommit_round: u64,
        source: TriggerSource,
        height: u64,
    ) -> ConsensusResult<()> {
        if self.height != height {
            return Ok(());
        }

        if self.step == Step::Commit {
            return Ok(());
        }

        log::debug!(
            "Tendermint: SMR triggered by precommit QC hash {:?} qc round {} from {:?}, height {}, round {}",
            hex_encode(precommit_hash.clone()),
            precommit_round,
            source,
            self.height,
            self.round
        );

        let (lock_round, lock_proposal) = self
            .lock
            .clone()
            .map_or_else(|| (None, None), |lock| (Some(lock.round), Some(lock.hash)));

        if precommit_hash.is_empty() {
            if precommit_round < self.round {
                return Ok(());
            }

            self.round = precommit_round;
            self.send_event(SMREvent::NewRoundInfo {
                height: self.height,
                round: self.round + 1,
                lock_round,
                lock_proposal,
                new_interval: None,
                new_config: None,
                from_where: FromWhere::PrecommitQC(precommit_round),
            })?;

            self.goto_next_round();
            return Ok(());
        }

        self.check()?;
        self.send_event(SMREvent::Commit(precommit_hash))?;
        self.goto_step(Step::Commit);
        Ok(())
    }

    fn handle_continue_round(&mut self, height: u64, round: u64) -> ConsensusResult<()> {
        if height != self.height || round <= self.round {
            return Ok(());
        }

        log::debug!("Tendermint: SMR continue round {}", round);

        self.round = round - 1;
        let (lock_round, lock_proposal) = self
            .lock
            .clone()
            .map_or_else(|| (None, None), |lock| (Some(lock.round), Some(lock.hash)));
        self.send_event(SMREvent::NewRoundInfo {
            height: self.height,
            round: self.round + 1,
            lock_round,
            lock_proposal,
            new_interval: None,
            new_config: None,
            from_where: FromWhere::ChokeQC(round - 1),
        })?;
        self.goto_next_round();
        Ok(())
    }

    fn send_event(&mut self, event: SMREvent) -> ConsensusResult<()> {
        log::debug!("Tendermint: SMR throw {} event", event);
        self.event.0.unbounded_send(event.clone()).map_err(|err| {
            ConsensusError::ThrowEventErr(format!("event: {}, error: {:?}", event.clone(), err))
        })?;
        self.event.1.unbounded_send(event.clone()).map_err(|err| {
            ConsensusError::ThrowEventErr(format!("event: {}, error: {:?}", event.clone(), err))
        })?;
        Ok(())
    }

    /// Goto new height and clear everything.
    fn goto_new_height(&mut self, height: u64) {
        log::debug!("Tendermint: SMR goto new height: {}", height);
        self.height = height;
        self.round = INIT_ROUND;
        self.block_hash = Hash::new();
        self.lock = None;
    }

    /// Keep the lock, if any, when go to the next round.
    fn goto_next_round(&mut self) {
        log::debug!("Tendermint: SMR goto next round {}", self.round + 1);
        self.round += 1;
        self.goto_step(Step::Propose);
    }

    /// Goto the given step.
    #[inline]
    fn goto_step(&mut self, step: Step) {
        log::debug!("Tendermint: SMR goto step {:?}", step);
        self.step = step;
    }

    /// Update the PoLC. Firstly set self proposal as the given hash. Secondly update the PoLC. If
    /// the hash is empty, remove it. Otherwise, set lock round and hash as the given round and
    /// hash.
    fn update_polc(&mut self, hash: Hash, round: u64) {
        log::debug!("Tendermint: SMR update PoLC at round {}", round);
        self.set_proposal(hash.clone());

        if hash.is_empty() {
            self.remove_polc();
        } else {
            self.lock = Some(Lock { round, hash });
        }
    }

    #[inline]
    fn remove_polc(&mut self) {
        self.lock = None;
    }

    /// Set self proposal hash as the given hash.
    #[inline]
    fn set_proposal(&mut self, proposal_hash: Hash) {
        self.block_hash = proposal_hash;
    }

    /// Do below self checks before each message is processed:
    /// 1. Whenever the lock is some and the proposal hash is empty, is impossible.
    /// 2. As long as there is a lock, the lock and proposal hash must be consistent.
    /// 3. Before precommit step, and round is 0, there can be no lock.
    /// 4. If the step is propose, proposal hash must be empty unless lock is some.
    #[inline(always)]
    fn check(&mut self) -> ConsensusResult<()> {
        log::debug!("Tendermint: SMR do self check");

        // // Lock hash must be same as proposal hash, if has.
        // if self.round == 0
        //     && self.lock.is_some()
        //     && self.lock.clone().unwrap().hash != self.block_hash
        // {
        //     return Err(ConsensusError::SelfCheckErr("Lock".to_string()));
        // }

        // // While self step lt precommit and round is 0, self lock must be none.
        // if self.step < Step::Precommit && self.round == 0 && self.lock.is_some() {
        //     return Err(ConsensusError::SelfCheckErr(format!(
        //         "Invalid lock, height {}, round {}",
        //         self.height, self.round
        //     )));
        // }

        // // While in precommit step, the lock and the proposal hash must be NOR.
        // if self.step == Step::Precommit &&
        // (self.block_hash.is_empty().bitxor(self.lock.is_none())) {
        //     return Err(ConsensusError::SelfCheckErr(format!(
        //         "Invalid status in precommit, height {}, round {}",
        //         self.height, self.round
        //     )));
        // }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use bytes::Bytes;
    use std::ops::BitXor;

    #[test]
    fn test_xor() {
        let left = Bytes::new();
        let right: Option<u64> = None;
        assert!(!left.is_empty().bitxor(&right.is_none()));
    }
}