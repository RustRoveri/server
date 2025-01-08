use rust_roveri_api::{FragmentId, SessionId};
use std::collections::{HashMap, VecDeque};
use wg_2024::{
    network::{NodeId, SourceRoutingHeader},
    packet::{Fragment, Packet, PacketType},
};

#[derive(Clone)]
pub struct ToBeSentFragment {
    pub dest: NodeId,
    pub session_id: SessionId,
    pub fragment: Fragment,
}

pub struct FragmentManager {
    cache: HashMap<FragmentId, ToBeSentFragment>,
    buffer: VecDeque<ToBeSentFragment>,
}

impl FragmentManager {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            buffer: VecDeque::new(),
        }
    }

    pub fn insert_fragment(&mut self, to_be_sent_fragment: ToBeSentFragment) {
        self.cache.insert(
            (
                to_be_sent_fragment.session_id,
                to_be_sent_fragment.fragment.fragment_index,
            ),
            to_be_sent_fragment.clone(),
        );

        self.buffer.push_back(to_be_sent_fragment);
    }

    pub fn get_next(&mut self) -> Option<ToBeSentFragment> {
        self.buffer.pop_front()
    }

    pub fn insert_from_cache(&mut self, fragment_id: FragmentId) -> Result<(), &str> {
        let to_be_sent_fragment = match self.cache.get(&fragment_id) {
            None => return Err("Requested fragment is not in cache"),
            Some(fragment) => fragment.clone(),
        };

        self.buffer.push_back(to_be_sent_fragment);

        Ok(())
    }

    pub fn remove_from_cache(&mut self, fragment_id: FragmentId) {
        self.cache.remove(&fragment_id);
    }
}
