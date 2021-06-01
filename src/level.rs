use std::{
    net::SocketAddr,
    sync::mpsc::{self, Receiver},
};

use bincode::serialize;
use crossbeam_channel::Sender;
use laminar::Packet;
use rg3d::{
    core::{
        algebra::Vector3,
        color::Color,
        pool::{Handle, Pool},
    },
    scene::{node::Node, ColliderHandle, Scene},
};

use crate::{
    message::{ActionMessage, NetworkMessage},
    player::{Player, PlayerState},
    GameEngine,
};

pub struct Level {
    pub scene: Handle<Scene>,
    players: Vec<Player>,
    receiver: Receiver<ActionMessage>,
    sender: mpsc::Sender<ActionMessage>,
}

impl Level {
    pub async fn new(
        engine: &mut GameEngine,
        scene_name: &str,
        receiver: Receiver<ActionMessage>,
        sender: mpsc::Sender<ActionMessage>,
    ) -> Self {
        // Make message queue.
        // let (_, receiver) = mpsc::channel();

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

        // Ask server to return new player and then spawn new player from network event

        Self {
            scene: engine.scenes.add(scene),
            players: Vec::new(),
            receiver,
            sender,
        }
    }

    pub fn get_player_by_index(&mut self, index: u32) -> Option<&mut Player> {
        self.players.iter_mut().find(|p| p.index == index)
    }

    pub fn get_player_by_collider(&mut self, collider: ColliderHandle) -> Option<&mut Player> {
        self.players.iter_mut().find(|p| p.collider == collider)
    }

    pub fn remove_player(&mut self, index: u32) {
        self.players.retain(|p| p.index != index)
    }

    pub fn update(
        &mut self,
        engine: &mut GameEngine,
        dt: f32,
        packet_sender: &Sender<Packet>,
        client_address: &mut String,
        elapsed_time: f32,
    ) {
        let scene = &mut engine.scenes[self.scene];

        while let Ok(action) = self.receiver.try_recv() {
            println!("action received: {:?}", action);
            match action {
                ActionMessage::ShootWeapon { index, active } => {
                    if let Some(player) = self.get_player_by_index(index) {
                        player.controller.shoot = active;
                    }
                }
                ActionMessage::MoveForward { index, active } => {
                    if let Some(player) = self.get_player_by_index(index) {
                        player.controller.move_forward = active;
                    }
                }
                ActionMessage::MoveBackward { index, active } => {
                    if let Some(player) = self.get_player_by_index(index) {
                        player.controller.move_backward = active;
                    }
                }
                ActionMessage::MoveLeft { index, active } => {
                    if let Some(player) = self.get_player_by_index(index) {
                        player.controller.move_left = active;
                    }
                }
                ActionMessage::MoveRight { index, active } => {
                    if let Some(player) = self.get_player_by_index(index) {
                        player.controller.move_right = active;
                    }
                }
                ActionMessage::MoveUp { index, active } => {
                    if let Some(player) = self.get_player_by_index(index) {
                        player.controller.move_up = active;
                    }
                }
                ActionMessage::LookAround {
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
                ActionMessage::UpdateState {
                    timestamp,
                    index,
                    x,
                    y,
                    z,
                    velocity,
                    yaw,
                    pitch,
                } => {
                    if let Some(player) = self.get_player_by_index(index) {
                        let new_state = PlayerState {
                            timestamp: timestamp,
                            position: Vector3::new(x, y, z),
                            vertical_velocity: velocity,
                            yaw: yaw,
                            pitch: pitch,
                        };
                        let previous_state = PlayerState {
                            timestamp: timestamp,
                            position: player.get_position(&scene),
                            vertical_velocity: player.get_vertical_velocity(&scene),
                            yaw: player.get_yaw(),
                            pitch: player.get_pitch(),
                        };
                        player.controller.previous_states.retain(|state| state.timestamp > timestamp);
                        player.controller.previous_states.push(previous_state);
                        player.controller.new_state = Some(new_state);
                    }
                }
                ActionMessage::DestroyBlock { index } => {
                    let handle = scene.graph.handle_from_index(index as usize);
                    // Empty tag means it is a block
                    if handle.is_some() && scene.graph[handle].tag().is_empty() {
                        if let Some(body) = scene.physics_binder.body_of(handle) {
                            scene.remove_node(handle);
                            scene.physics.remove_body(body.into());
                        }
                    }
                }
                ActionMessage::KillPlayer { index } => {
                    if let Some(player) = self.get_player_by_index(index) {
                        player.remove_nodes(scene);
                    }
                    self.remove_player(index);
                }
                _ => (),
            }
        }

        for player in self.players.iter_mut() {
            if cfg!(feature = "server") {
                let position = player.get_position(&scene);
                if let Ok(client_addr) = client_address.parse::<SocketAddr>() {
                    let state_message = NetworkMessage::Action {
                        index: player.index,
                        action: ActionMessage::UpdateState {
                            timestamp: elapsed_time,
                            index: player.index,
                            x: position.x,
                            y: position.y,
                            z: position.z,
                            velocity: player.get_vertical_velocity(&scene),
                            yaw: player.get_yaw(),
                            pitch: player.get_pitch(),
                        },
                    };

                    packet_sender
                        .send(Packet::unreliable_sequenced(
                            client_addr,
                            serialize(&state_message).unwrap(),
                            None,
                        ))
                        .unwrap();
                }
            }

            player.update(
                dt,
                scene,
                engine.resource_manager.clone(),
                packet_sender,
                client_address,
                &self.sender,
            );
        }
    }

    pub async fn spawn_player(
        &mut self,
        engine: &mut GameEngine,
        index: u32,
        position: Vector3<f32>,
        current_player: bool,
    ) {
        let scene = &mut engine.scenes[self.scene];

        let player = Player::new(
            scene,
            position,
            engine.resource_manager.clone(),
            current_player,
            index,
        )
        .await;

        let _ = self.players.push(player);
    }
}
