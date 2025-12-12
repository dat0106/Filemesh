// Import các thư viện cần thiết.
use anyhow::Result;
use libp2p::{gossipsub, PeerId};

/// Cấu trúc `Room` đại diện cho một phòng chat hoặc một kênh mà các peer có thể tham gia.
/// Nó quản lý chủ đề (topic) của gossipsub và các thông tin liên quan.
pub struct Room {
    /// Tên của phòng.
    name: String,
    /// Chủ đề (topic) của gossipsub, được tạo ra từ tên phòng.
    /// Các peer trong cùng một phòng sẽ đăng ký (subscribe) cùng một topic này.
    topic: gossipsub::IdentTopic,
    /// `PeerId` của peer cục bộ.
    local_peer_id: PeerId,
}

impl Room {
    /// Tạo một `Room` mới.
    pub fn new(name: String, local_peer_id: PeerId) -> Self {
        // Tạo một `IdentTopic` từ tên phòng. Đây là định danh duy nhất cho kênh gossipsub.
        // Tất cả các peer trong cùng phòng "filemesh-room-<tên_phòng>" sẽ có thể giao tiếp với nhau.
        let topic = gossipsub::IdentTopic::new(format!("filemesh-room-{}", name));
        Room {
            name,
            topic,
            local_peer_id,
        }
    }

    /// Trả về tên của phòng.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Trả về topic gossipsub của phòng.
    pub fn topic(&self) -> &gossipsub::IdentTopic {
        &self.topic
    }

    /// Trả về `PeerId` của peer cục bộ.
    pub fn local_peer_id(&self) -> &PeerId {
        &self.local_peer_id
    }

    /// Cho phép peer tham gia (subscribe) vào topic của phòng.
    /// Sau khi tham gia, peer sẽ bắt đầu nhận được các tin nhắn được quảng bá trên topic này.
    pub fn join(&self, gossipsub: &mut gossipsub::Behaviour) -> Result<()> {
        gossipsub.subscribe(&self.topic)?;
        Ok(())
    }

    /// Quảng bá (publish) một tin nhắn đến tất cả các peer khác trong cùng phòng.
    pub fn broadcast(&self, gossipsub: &mut gossipsub::Behaviour, message: &[u8]) -> Result<()> {
        match gossipsub.publish(self.topic.clone(), message) {
            // Lỗi `InsufficientPeers` là bình thường khi mới khởi động và chưa có peer nào,
            // vì vậy chúng ta không coi đó là một lỗi thực sự.
            Err(gossipsub::PublishError::InsufficientPeers) => Ok(()),
            Err(e) => Err(e.into()),
            Ok(_) => Ok(()),
        }
    }

    /// Cho phép peer rời khỏi (unsubscribe) topic của phòng.
    /// Sau khi rời đi, peer sẽ không còn nhận được tin nhắn từ phòng này nữa.
    pub fn leave(&self, gossipsub: &mut gossipsub::Behaviour) -> Result<()> {
        gossipsub.unsubscribe(&self.topic)?;
        Ok(())
    }
}
