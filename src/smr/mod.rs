///
pub mod smr_types;
///
mod state_machine;

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::channel::mpsc::{UnboundedReceiver};
use futures::stream::{FusedStream, Stream, StreamExt};

use crate::smr::smr_types::{SMREvent};

///
#[derive(Debug)]
pub struct Event {
    rx: UnboundedReceiver<SMREvent>,
}

impl Stream for Event {
    type Item = SMREvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        self.rx.poll_next_unpin(cx)
    }
}

impl FusedStream for Event {
    fn is_terminated(&self) -> bool {
        self.rx.is_terminated()
    }
}

impl Event {
    pub fn new(receiver: UnboundedReceiver<SMREvent>) -> Self {
        Event { rx: receiver }
    }
}

#[cfg(test)]
mod test {
    use futures::StreamExt;

    use crate::smr::smr_types::{SMRStatus, SMRTrigger, TriggerSource, TriggerType};
    use crate::types::{Hash, INIT_HEIGHT, INIT_ROUND};

    use super::{state_machine::StateMachine};

    #[tokio::test]
    async fn test_smr() {
        let (mut smr, mut rx_state, _rx_timer) = StateMachine::new();

        let status = SMRStatus::new(INIT_HEIGHT + 1);
        let msg = SMRTrigger {
            trigger_type: TriggerType::NewHeight(status),
            source: TriggerSource::State,
            hash: Hash::new(),
            lock_round: None,
            round: INIT_ROUND,
            height: INIT_HEIGHT,
        };
        match smr.process(msg) {
            Ok(_) => println!("success"),
            Err(e) => println!("error: {:?}", e),
        }
        match rx_state.next().await {
            Some(event) => {
                println!("{:?}", event);

            }
            None => println!("none"),
        }
        
    }
}
