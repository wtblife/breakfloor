use std::sync::mpsc::{self, Receiver, Sender};

use serde::{Deserialize, Serialize};

use crate::{
    level::{Level, LevelState},
    network_manager::{NetworkManager, NetworkMessage},
    GameEngine, Settings,
};

pub struct Game {
    pub level: Option<Level>,
    pub event_sender: Sender<GameEvent>,
    pub event_receiver: Receiver<GameEvent>,
    pub server: bool,
    pub settings: Settings,
}

impl Game {
    pub async fn new(engine: &mut GameEngine, settings: Settings) -> Self {
        let (event_sender, event_receiver) = mpsc::channel();

        let server = cfg!(feature = "server");

        let mut level = None;
        #[cfg(feature = "server")]
        {
            level = Some(
                Level::new(
                    engine,
                    "block_scene",
                    LevelState {
                        destroyed_blocks: Vec::new(),
                    },
                )
                .await,
            );
        }

        Self {
            level,
            event_sender,
            event_receiver,
            server,
            settings,
        }
    }

    pub fn update(
        &mut self,
        engine: &mut GameEngine,
        dt: f32,
        network_manager: &mut NetworkManager,
        elapsed_time: f32,
    ) {
        while let Ok(event) = self.event_receiver.try_recv() {
            match event {
                GameEvent::Connected => (),
                GameEvent::LoadLevel { level, state } => {
                    let level =
                        rg3d::futures::executor::block_on(Level::new(engine, &level, state));
                    self.level = Some(level);

                    network_manager.send_to_server_reliably(&NetworkMessage::GameEvent {
                        event: GameEvent::Joined,
                    });
                }
                // Only received on server
                GameEvent::Joined => (),
                GameEvent::Disconnected => (),
            }
        }

        if let Some(level) = &mut self.level {
            level.update(engine, dt, network_manager, elapsed_time);
        }
    }

    pub fn queue_event(&mut self, event: GameEvent) {
        self.event_sender.send(event).unwrap();
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum GameEvent {
    Connected,
    Disconnected,
    LoadLevel {
        level: String, // Sent from server to tell client what to load
        state: LevelState,
    },
    Joined,
}
