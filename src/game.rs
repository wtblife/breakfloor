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
    pub active: bool,
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
            active: true,
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
                    let new_level = rg3d::futures::executor::block_on(Level::new(
                        engine,
                        &level,
                        state.clone(),
                    ));
                    if let Some(level) = &mut self.level {
                        level.clean_up(engine);
                    }
                    self.level = Some(new_level);

                    #[cfg(not(feature = "server"))]
                    network_manager.send_to_server_reliably(&NetworkMessage::GameEvent {
                        event: GameEvent::Joined,
                    });

                    #[cfg(feature = "server")]
                    network_manager.send_to_all_reliably(&NetworkMessage::GameEvent {
                        event: GameEvent::LoadLevel {
                            level,
                            state: state.clone(),
                        },
                    });
                }
                #[cfg(feature = "server")]
                GameEvent::Joined => {}
                #[cfg(not(feature = "server"))]
                GameEvent::Disconnected => {
                    self.active = false;
                }
                _ => (),
            }
        }

        if let Some(level) = &mut self.level {
            level.update(
                engine,
                dt,
                network_manager,
                elapsed_time,
                &self.event_sender,
            );
        }
    }

    pub fn queue_event(&self, event: GameEvent) {
        self.event_sender.send(event).unwrap();
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum GameEvent {
    Connected,
    Disconnected,
    LoadLevel {
        level: String, // Sent from server to tell client what to load
        state: LevelState,
    },
    Joined,
}
