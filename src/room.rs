use anyhow::Result;
use libp2p::{gossipsub, PeerId};

pub struct Room {
    name: String,
    topic: gossipsub::IdentTopic,
    local_peer_id: PeerId,
}

impl Room {
    pub fn new(name: String, local_peer_id: PeerId) -> Self {
        let topic = gossipsub::IdentTopic::new(format!("filemesh-room-{}", name));
        Room {
            name,
            topic,
            local_peer_id,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn topic(&self) -> &gossipsub::IdentTopic {
        &self.topic
    }

    pub fn local_peer_id(&self) -> &PeerId {
        &self.local_peer_id
    }

    pub fn join(&self, gossipsub: &mut gossipsub::Behaviour) -> Result<()> {
        gossipsub.subscribe(&self.topic)?;
        Ok(())
    }

    pub fn broadcast(&self, gossipsub: &mut gossipsub::Behaviour, message: &[u8]) -> Result<()> {
        match gossipsub.publish(self.topic.clone(), message) {
            // It's normal to have no peers right after startup; don't treat this as an error.
            Err(gossipsub::PublishError::InsufficientPeers) => Ok(()),
            Err(e) => Err(e.into()),
            Ok(_) => Ok(()),
        }
    }

    pub fn leave(&self, gossipsub: &mut gossipsub::Behaviour) -> Result<()> {
        gossipsub.unsubscribe(&self.topic)?;
        Ok(())
    }
}
