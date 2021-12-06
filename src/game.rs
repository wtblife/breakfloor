use std::sync::{
    mpsc::{self, Receiver, Sender},
    Arc, Mutex,
};

use rg3d::scene::Scene;
use serde::{Deserialize, Serialize};

use crate::{
    level::{Level, LevelState},
    network_manager::{NetworkManager, NetworkMessage},
    GameEngine, Interface, Settings,
};

pub struct LoadContext {
    level: Option<((Level, Scene), LevelState)>,
}

pub struct Game {
    pub level: Option<Level>,
    pub event_sender: Sender<GameEvent>,
    pub event_receiver: Receiver<GameEvent>,
    pub server: bool,
    pub settings: Settings,
    pub active: bool,
    load_context: Option<Arc<Mutex<LoadContext>>>,
}

impl Game {
    pub async fn new(engine: &mut GameEngine, settings: Settings) -> Self {
        let (event_sender, event_receiver) = mpsc::channel();
        let resource_manager = engine.resource_manager.clone();

        let ctx = Arc::new(Mutex::new(LoadContext { level: None }));
        let load_context = Some(ctx.clone());

        let server = cfg!(feature = "server");

        // TODO: Replace this with an event to load level?
        #[cfg(feature = "server")]
        {
            std::thread::spawn(move || {
                let state = LevelState {
                    destroyed_blocks: Vec::new(),
                };
                let level = rg3d::core::futures::executor::block_on(Level::new(
                    resource_manager,
                    "block_remake",
                    LevelState {
                        destroyed_blocks: Vec::new(),
                    },
                ));

                ctx.lock().unwrap().level = Some((level, state));
            });
        }

        Self {
            level: None,
            event_sender,
            event_receiver,
            server,
            settings,
            active: true,
            load_context: load_context,
        }
    }

    pub fn update(
        &mut self,
        engine: &mut GameEngine,
        dt: f32,
        network_manager: &mut NetworkManager,
        elapsed_time: f32,
        interface: &Interface,
    ) {
        while let Ok(event) = self.event_receiver.try_recv() {
            // println!("game event received: {:?}", event);
            match event {
                GameEvent::Connected => (),
                GameEvent::LoadLevel { level, state } => {
                    let resource_manager = engine.resource_manager.clone();

                    let ctx = Arc::new(Mutex::new(LoadContext { level: None }));

                    self.load_context = Some(ctx.clone());

                    std::thread::spawn(move || {
                        let level = rg3d::core::futures::executor::block_on(Level::new(
                            resource_manager,
                            level.as_str(),
                            state.clone(),
                        ));

                        ctx.lock().unwrap().level = Some((level, state));

                        // Scene will be loaded in separate thread.
                    });
                }
                GameEvent::LoadedLevel => {}
                #[cfg(feature = "server")]
                GameEvent::Joined => {}
                #[cfg(not(feature = "server"))]
                GameEvent::Disconnected => {
                    self.active = false;
                }
                _ => (),
            }
        }

        if let Some(ctx) = self.load_context.clone() {
            if let Ok(mut ctx) = ctx.try_lock() {
                if let Some(((mut new_level, scene), state)) = ctx.level.take() {
                    if let Some(old_level) = &mut self.level {
                        old_level.clean_up(engine);
                    }

                    #[cfg(not(feature = "server"))]
                    network_manager.send_to_server_reliably(&NetworkMessage::GameEvent {
                        event: GameEvent::Joined,
                    });

                    #[cfg(feature = "server")]
                    network_manager.send_to_all_reliably(&NetworkMessage::GameEvent {
                        event: GameEvent::LoadLevel {
                            level: new_level.name.clone(),
                            state: new_level.state.clone(),
                        },
                    });

                    new_level.scene = engine.scenes.add(scene);
                    new_level.apply_state(engine, state);
                    self.level = Some(new_level);
                    self.load_context = None;

                    // #[cfg(feature = "server")]
                    // self.set_menu_visible(false);
                    // self.engine
                    //     .user_interface
                    //     .send_message(WidgetMessage::visibility(
                    //         self.loading_screen.root,
                    //         MessageDirection::ToWidget,
                    //         false,
                    //     ));
                    // self.menu.sync_to_model(&mut self.engine, true);
                } else {
                    // self.loading_screen.set_progress(
                    //     &self.engine.user_interface,
                    //     self.engine.resource_manager.state().loading_progress() as f32 / 100.0,
                    // );
                }
            }
        }

        if let Some(level) = &mut self.level {
            level.update(
                engine,
                dt,
                network_manager,
                elapsed_time,
                &self.event_sender,
                interface,
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
    LoadedLevel,
    Joined,
}
