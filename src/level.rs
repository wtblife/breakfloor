use std::{
    sync::mpsc::{self, Receiver},
    thread::current,
};

use rg3d::{
    core::{
        algebra::Vector3,
        color::Color,
        pool::{Handle, Pool},
    },
    scene::{node::Node, Scene},
};

use crate::{message::Message, player::Player, GameEngine};

pub struct Level {
    scene: Handle<Scene>,
    players: Pool<Player>,
    receiver: Receiver<Message>, // current_player: Option<Handle<Node>>,
}

impl Level {
    pub async fn new(
        engine: &mut GameEngine,
        scene_name: &str,
        receiver: Receiver<Message>,
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

    fn find_player(&mut self, index: u32) -> Option<&mut Player> {
        self.players.iter_mut().find(|p| p.index == index)
    }

    pub fn update(&mut self, engine: &mut GameEngine, dt: f32) {
        let scene = &mut engine.scenes[self.scene];

        while let Ok(message) = self.receiver.try_recv() {
            match message {
                Message::ShootWeapon { index, active } => {
                    if let Some(player) = self.find_player(index) {
                        player.controller.shoot = active;
                    }
                }
                Message::MoveForward { index, active } => {
                    if let Some(player) = self.find_player(index) {
                        player.controller.move_forward = active;
                    }
                }
                Message::MoveBackward { index, active } => {
                    if let Some(player) = self.find_player(index) {
                        player.controller.move_backward = active;
                    }
                }
                Message::MoveLeft { index, active } => {
                    if let Some(player) = self.find_player(index) {
                        player.controller.move_left = active;
                    }
                }
                Message::MoveRight { index, active } => {
                    if let Some(player) = self.find_player(index) {
                        player.controller.move_right = active;
                    }
                }
                Message::MoveUp { index, active } => {
                    if let Some(player) = self.find_player(index) {
                        player.controller.move_up = active;
                    }
                }
                Message::LookAround {
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
                _ => (),
            }
        }

        for player in self.players.iter_mut() {
            player.update(dt, scene, engine.resource_manager.clone());
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
