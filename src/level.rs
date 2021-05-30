use std::sync::mpsc::{self, Receiver};

use crossbeam_channel::Sender;
use laminar::Packet;
use rg3d::{
    core::{
        algebra::Vector3,
        color::Color,
        pool::{Handle, Pool},
    },
    scene::{node::Node, Scene},
};

use crate::{
    message::{ActionMessage, NetworkMessage},
    player::{Player, PlayerState},
    GameEngine,
};

pub struct Level {
    pub scene: Handle<Scene>,
    players: Pool<Player>,
    receiver: Receiver<ActionMessage>,
}

impl Level {
    pub async fn new(
        engine: &mut GameEngine,
        scene_name: &str,
        receiver: Receiver<ActionMessage>,
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

        scene.ambient_lighting_color = Color::opaque(255, 255, 255);

        // Disable SSAO

        // Ask server to return new player and then spawn new player from network event

        Self {
            scene: engine.scenes.add(scene),
            players: Pool::new(),
            receiver,
        }
    }

    pub fn find_player(&mut self, index: u32) -> Option<&mut Player> {
        self.players.iter_mut().find(|p| p.index == index)
    }

    pub fn update(
        &mut self,
        engine: &mut GameEngine,
        dt: f32,
        packet_sender: &Sender<Packet>,
        client_address: &mut String,
    ) {
        let scene = &mut engine.scenes[self.scene];

        while let Ok(action) = self.receiver.try_recv() {
            println!("action received: {:?}", action);
            match action {
                ActionMessage::ShootWeapon { index, active } => {
                    if let Some(player) = self.find_player(index) {
                        player.controller.shoot = active;
                    }
                }
                ActionMessage::MoveForward { index, active } => {
                    if let Some(player) = self.find_player(index) {
                        player.controller.move_forward = active;
                    }
                }
                ActionMessage::MoveBackward { index, active } => {
                    if let Some(player) = self.find_player(index) {
                        player.controller.move_backward = active;
                    }
                }
                ActionMessage::MoveLeft { index, active } => {
                    if let Some(player) = self.find_player(index) {
                        player.controller.move_left = active;
                    }
                }
                ActionMessage::MoveRight { index, active } => {
                    if let Some(player) = self.find_player(index) {
                        player.controller.move_right = active;
                    }
                }
                ActionMessage::MoveUp { index, active } => {
                    if let Some(player) = self.find_player(index) {
                        player.controller.move_up = active;
                    }
                }
                ActionMessage::LookAround {
                    index,
                    yaw_delta,
                    pitch_delta,
                } => {
                    if let Some(player) = self.find_player(index) {
                        player.controller.yaw -= yaw_delta;
                        player.controller.pitch =
                            (player.controller.pitch + pitch_delta).clamp(-90.0, 90.0);
                    }
                }
                ActionMessage::UpdateState {
                    index,
                    x,
                    y,
                    z,
                    velocity,
                    yaw,
                    pitch,
                } => {
                    if let Some(player) = self.find_player(index) {
                        player.controller.new_state = Some(PlayerState {
                            position: Vector3::new(x, y, z),
                            velocity: velocity,
                            yaw: yaw,
                            pitch: pitch,
                        });
                    }
                }
                ActionMessage::UpdatePreviousState { index } => {
                    if let Some(player) = self.find_player(index) {
                        // TODO: Does state need to be indexed? Could it be the wrong states being compared if multiple actions took place on client, but server hasn't sent a response yet?
                        let position = player.get_position(&scene);
                        player.controller.previous_state = PlayerState {
                            position,
                            velocity: player.get_vertical_velocity(&scene),
                            yaw: player.get_yaw(),
                            pitch: player.get_pitch(),
                        };
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
                _ => (),
            }
        }

        for player in self.players.iter_mut() {
            player.update(
                dt,
                scene,
                engine.resource_manager.clone(),
                packet_sender,
                client_address,
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

        let _ = self.players.spawn(player);
    }
}
