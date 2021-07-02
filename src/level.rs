use std::{
    net::SocketAddr,
    sync::mpsc::{self, channel, Receiver, Sender},
    thread::spawn,
};

use rg3d::{
    core::{
        algebra::Vector3,
        color::Color,
        pool::{Handle, Pool},
    },
    scene::{node::Node, ColliderHandle, Scene},
};
use serde::{Deserialize, Serialize};

use crate::{
    game::GameEvent,
    network_manager::{NetworkManager, NetworkMessage},
    player::{self, Player, PlayerState},
    player_event::{PlayerEvent, SerializablePlayerState, SerializableVector},
    GameEngine,
};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LevelState {
    pub destroyed_blocks: Vec<u32>,
}

pub struct Level {
    pub scene: Handle<Scene>,
    pub name: String,
    players: Vec<Player>,
    receiver: Receiver<PlayerEvent>,
    pub sender: Sender<PlayerEvent>,
    pub state: LevelState,
}

impl Level {
    pub async fn new(engine: &mut GameEngine, scene_name: &str, state: LevelState) -> Self {
        let mut scene = Scene::new();

        // engine
        //     .resource_manager
        //     .state()
        //     .set_textures_path("data/textures");

        // Load a scene resource and create its instance.
        engine
            .resource_manager
            .request_model(["data/levels/", scene_name, ".rgs"].concat())
            .await
            .unwrap()
            .instantiate_geometry(&mut scene);

        for (handle, node) in scene.graph.pair_iter() {
            if let Some(body_handle) = scene.physics_binder.body_of(handle) {
                if let Some(body) = scene.physics.bodies.get(body_handle.into()) {
                    for &collider_handle in body.colliders().iter() {
                        scene.physics.colliders[collider_handle].friction = 0.0;
                    }
                }
            }
        }

        scene.ambient_lighting_color = Color::opaque(255, 255, 255);

        let (sender, receiver) = channel();

        let mut level = Self {
            name: String::from(scene_name),
            scene: engine.scenes.add(scene),
            players: Vec::new(),
            receiver: receiver,
            sender: sender,
            state: LevelState {
                destroyed_blocks: Vec::new(),
            },
        };

        level.apply_state(engine, state);

        level
    }

    pub fn get_player_by_index(&mut self, index: u32) -> Option<&mut Player> {
        self.players.iter_mut().find(|p| p.index == index)
    }

    pub fn get_player_by_collider(&self, collider: ColliderHandle) -> Option<&Player> {
        self.players.iter().find(|p| p.collider == collider)
    }

    pub fn remove_player(&mut self, engine: &mut GameEngine, index: u32) {
        let scene = &mut engine.scenes[self.scene];
        if let Some(player) = self.get_player_by_index(index) {
            player.clean_up(scene);
        }

        self.players.retain(|p| p.index != index)
    }

    pub fn clean_up(&mut self, engine: &mut GameEngine) {
        let scene = &mut engine.scenes[self.scene];

        for player in self.players.iter_mut() {
            player.clean_up(scene);
        }

        self.players.clear();
    }

    pub fn update(
        &mut self,
        engine: &mut GameEngine,
        dt: f32,
        network_manager: &mut NetworkManager,
        elapsed_time: f32,
        game_event_sender: &Sender<GameEvent>,
    ) {
        while let Ok(action) = self.receiver.try_recv() {
            // if let PlayerEvent::UpdateState { .. } = action {
            // } else {
            //     println!("player event received: {:?}", action);
            // };

            match action {
                PlayerEvent::ShootWeapon {
                    index,
                    active,
                    yaw,
                    pitch,
                } => {
                    if let Some(player) = self.get_player_by_index(index) {
                        player.controller.shoot = active;

                        if network_manager
                            .player_index
                            .and_then(|id| if id == index { Some(id) } else { None })
                            .is_none()
                        {
                            player.controller.yaw = yaw;
                            player.controller.pitch = pitch;
                        }
                    }
                }
                PlayerEvent::MoveForward {
                    index,
                    active,
                    yaw,
                    pitch,
                } => {
                    if let Some(player) = self.get_player_by_index(index) {
                        player.controller.move_forward = active;

                        if network_manager
                            .player_index
                            .and_then(|id| if id == index { Some(id) } else { None })
                            .is_none()
                        {
                            player.controller.yaw = yaw;
                            player.controller.pitch = pitch;
                        }
                    }
                }
                PlayerEvent::MoveBackward {
                    index,
                    active,
                    yaw,
                    pitch,
                } => {
                    if let Some(player) = self.get_player_by_index(index) {
                        player.controller.move_backward = active;

                        if network_manager
                            .player_index
                            .and_then(|id| if id == index { Some(id) } else { None })
                            .is_none()
                        {
                            player.controller.yaw = yaw;
                            player.controller.pitch = pitch;
                        }
                    }
                }
                PlayerEvent::MoveLeft {
                    index,
                    active,
                    yaw,
                    pitch,
                } => {
                    if let Some(player) = self.get_player_by_index(index) {
                        player.controller.move_left = active;

                        if network_manager
                            .player_index
                            .and_then(|id| if id == index { Some(id) } else { None })
                            .is_none()
                        {
                            player.controller.yaw = yaw;
                            player.controller.pitch = pitch;
                        }
                    }
                }
                PlayerEvent::MoveRight {
                    index,
                    active,
                    yaw,
                    pitch,
                } => {
                    if let Some(player) = self.get_player_by_index(index) {
                        player.controller.move_right = active;

                        if network_manager
                            .player_index
                            .and_then(|id| if id == index { Some(id) } else { None })
                            .is_none()
                        {
                            player.controller.yaw = yaw;
                            player.controller.pitch = pitch;
                        }
                    }
                }
                PlayerEvent::MoveUp { index, active } => {
                    if let Some(player) = self.get_player_by_index(index) {
                        player.controller.move_up = active;
                    } else {
                        // TODO: Handle respawn with it's own event
                        // #[cfg(feature = "server")]
                        // if let Some(address) = network_manager.get_address_for_player(index) {
                        //     // If one or less players left, restart level

                        //     let position = SerializableVector {
                        //         x: 1.5 + 3.0 * (-1.0f32).powi(index as i32),
                        //         y: 1.0,
                        //         z: 0.0,
                        //     };
                        //     let spawn_event = PlayerEvent::SpawnPlayer {
                        //         index: index,
                        //         state: SerializablePlayerState {
                        //             position: position,
                        //             ..Default::default()
                        //         },
                        //         current_player: false,
                        //     };
                        //     self.queue_event(spawn_event);
                        //     network_manager.send_to_all_except_address_reliably(
                        //         address,
                        //         &NetworkMessage::PlayerEvent {
                        //             index: index,
                        //             event: spawn_event,
                        //         },
                        //     );

                        //     let spawn_event = PlayerEvent::SpawnPlayer {
                        //         index: index,
                        //         state: SerializablePlayerState {
                        //             position: position,
                        //             ..Default::default()
                        //         },
                        //         current_player: true,
                        //     };
                        //     network_manager.send_to_address_reliably(
                        //         address,
                        //         &NetworkMessage::PlayerEvent {
                        //             index: index,
                        //             event: spawn_event,
                        //         },
                        //     );
                        // }
                    }
                }
                PlayerEvent::LookAround {
                    index,
                    yaw_delta,
                    pitch_delta,
                } => {
                    if let Some(player) = self.get_player_by_index(index) {
                        player.controller.yaw -= yaw_delta;
                        player.controller.pitch =
                            (player.controller.pitch + pitch_delta).clamp(-90.0, 90.0);
                    }
                }
                PlayerEvent::UpdateState {
                    timestamp,
                    index,
                    position,
                    velocity,
                    yaw,
                    pitch,
                    shoot,
                    fuel,
                } => {
                    let scene = &mut engine.scenes[self.scene];
                    if let Some(player) = self.get_player_by_index(index) {
                        let new_state = PlayerState {
                            timestamp: timestamp,
                            position: Vector3::new(position.x, position.y, position.z),
                            velocity: Vector3::new(velocity.x, velocity.y, velocity.z),
                            yaw: yaw,
                            pitch: pitch,
                            shoot: shoot,
                            fuel: fuel,
                        };
                        let previous_state = PlayerState {
                            timestamp: timestamp,
                            position: player.get_position(scene),
                            velocity: player.get_velocity(scene),
                            yaw: player.get_yaw(),
                            pitch: player.get_pitch(),
                            shoot: player.controller.shoot,
                            fuel: player.flight_fuel,
                        };
                        player
                            .controller
                            .previous_states
                            .retain(|state| state.timestamp > timestamp);
                        player.controller.previous_states.push(previous_state);
                        player.controller.new_state = Some(new_state);
                    }
                }
                PlayerEvent::DestroyBlock { index } => {
                    self.destroy_block(engine, index);
                }
                #[cfg(feature = "server")]
                PlayerEvent::KillPlayerFromIntersection { collider } => {
                    // If player was killed then send kill and respawn events
                    if let Some(player) = self.get_player_by_collider(collider) {
                        let kill_event = PlayerEvent::KillPlayer {
                            index: player.index,
                        };
                        let kill_message = NetworkMessage::PlayerEvent {
                            index: player.index,
                            event: kill_event,
                        };

                        network_manager.send_to_all_reliably(&kill_message);
                        self.queue_event(kill_event);

                        if self.players.len() < 3 {
                            let event = GameEvent::LoadLevel {
                                level: self.name.clone(),
                                state: LevelState {
                                    destroyed_blocks: Vec::new(),
                                },
                            };
                            game_event_sender.send(event).unwrap();
                        }
                    }
                }
                PlayerEvent::KillPlayer { index } => {
                    self.remove_player(engine, index);
                    // If current player was killed then spectate another player
                    if let Some(player_index) = network_manager.player_index {
                        if player_index == index {
                            let scene = &mut engine.scenes[self.scene];
                            if let Some(player_to_spectate) = self.players.first() {
                                player_to_spectate.set_camera(scene, true);
                            }
                        }
                    }
                }
                PlayerEvent::SpawnPlayer {
                    index,
                    state,
                    current_player,
                } => {
                    rg3d::futures::executor::block_on(self.spawn_player(
                        engine,
                        index,
                        PlayerState {
                            position: Vector3::new(
                                state.position.x,
                                state.position.y,
                                state.position.z,
                            ),
                            velocity: Vector3::new(
                                state.velocity.x,
                                state.velocity.y,
                                state.velocity.z,
                            ),
                            yaw: state.yaw,
                            pitch: state.pitch,
                            shoot: state.shoot,
                            ..Default::default()
                        },
                        current_player,
                        network_manager,
                    ));
                }
                _ => (),
            }
        }

        let scene = &mut engine.scenes[self.scene];
        for player in self.players.iter_mut() {
            #[cfg(feature = "server")]
            {
                let position = player.get_position(&scene);
                let velocity = player.get_velocity(&scene);
                let state_message = NetworkMessage::PlayerEvent {
                    index: player.index,
                    event: PlayerEvent::UpdateState {
                        timestamp: elapsed_time,
                        index: player.index,
                        position: SerializableVector {
                            x: position.x,
                            y: position.y,
                            z: position.z,
                        },
                        velocity: SerializableVector {
                            x: velocity.x,
                            y: velocity.y,
                            z: velocity.z,
                        },
                        yaw: player.get_yaw(),
                        pitch: player.get_pitch(),
                        shoot: player.controller.shoot,
                        fuel: player.flight_fuel,
                    },
                };

                network_manager.send_to_all_unreliably(&state_message, 0);
            }

            player.update(
                dt,
                scene,
                engine.resource_manager.clone(),
                network_manager,
                &self.sender,
            );
        }
    }

    pub async fn spawn_player(
        &mut self,
        engine: &mut GameEngine,
        index: u32,
        state: PlayerState,
        current_player: bool,
        network_manager: &mut NetworkManager,
    ) {
        let scene = &mut engine.scenes[self.scene];

        if self.get_player_by_index(index).is_none() {
            if current_player {
                network_manager.player_index = Some(index);

                // Disable any spectator cams
                for existing_player in self.players.iter() {
                    existing_player.set_camera(scene, false);
                }
            }

            let player = Player::new(
                scene,
                state,
                engine.resource_manager.clone(),
                current_player,
                index,
            )
            .await;

            self.players.push(player);
        }
    }

    // Call on clients to load level state
    pub fn apply_state(&mut self, engine: &mut GameEngine, state: LevelState) {
        for i in state.destroyed_blocks {
            self.destroy_block(engine, i);
        }
    }

    pub fn destroy_block(&mut self, engine: &mut GameEngine, index: u32) {
        let scene = &mut engine.scenes[self.scene];

        let handle = scene.graph.handle_from_index(index as usize);

        if handle.is_some() && scene.graph.is_valid_handle(handle) {
            if let Some(body) = scene.physics_binder.body_of(handle) {
                scene.remove_node(handle);
                scene.physics.remove_body(body.into());

                #[cfg(feature = "server")]
                self.state.destroyed_blocks.push(index);
            }
        }
    }

    pub fn players(&self) -> &Vec<Player> {
        &self.players
    }

    pub fn queue_event(&self, event: PlayerEvent) {
        self.sender.send(event).unwrap();
    }
}
