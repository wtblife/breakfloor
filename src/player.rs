use bincode::serialize;
use fyrox::{
    animation::Animation,
    core::{
        algebra::{Matrix3, Translation3, UnitQuaternion, Vector3},
        color::Color,
        color_gradient::{ColorGradient, GradientPoint},
        math::{ray::Ray, Vector3Ext},
        pool::Handle,
    },
    engine::resource_manager::ResourceManager,
    event::ElementState,
    gui::{message::MessageDirection, text::TextMessage},
    material::{Material, PropertyValue},
    resource::texture::TextureWrapMode,
    scene::{
        base::BaseBuilder,
        camera::{CameraBuilder, Exposure, SkyBox, SkyBoxBuilder},
        collider::{ColliderBuilder, ColliderShape},
        graph::{
            physics::{CoefficientCombineRule, RayCastOptions},
            Graph,
        },
        mesh::{
            surface::{SurfaceBuilder, SurfaceData},
            MeshBuilder, RenderPath,
        },
        node::Node,
        particle_system::ParticleSystemBuilder,
        rigidbody::{RigidBody, RigidBodyBuilder},
        sound::{listener::ListenerBuilder, SoundBufferResource, SoundBuilder, Status},
        transform::TransformBuilder,
        Scene,
    },
};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::mpsc::{self, Sender},
};

use crate::{
    animation::{PlayerAnimationMachine, PlayerAnimationMachineInput},
    level::Level,
    network_manager::{self, NetworkManager, NetworkMessage},
    player_event::PlayerEvent,
    GameEngine, Interface,
};

const MOVEMENT_SPEED: f32 = 1.5;
const GRAVITY_SCALE: f32 = 0.6;
const JET_SPEED: f32 = 0.0155;
const JUMP_SCALAR: f32 = 0.32;
const MAX_FUEL: u32 = 225;
pub const SYNC_FREQUENCY: u32 = 3;

#[derive(Default)]
pub struct PlayerController {
    pub move_forward: bool,
    pub move_backward: bool,
    pub move_left: bool,
    pub move_right: bool,
    pub move_up: bool,
    pub jump: bool,
    pub fly: bool,
    pub pitch: f32,
    pub yaw: f32,
    pub dest_pitch: f32,
    pub dest_yaw: f32,
    pub shoot: bool,
    pub new_states: Vec<PlayerState>,
    pub previous_states: Vec<PlayerState>,
    pub smoothing_speed: f32,
}

pub struct Player {
    barrel: Handle<Node>,
    spine: Handle<Node>,
    camera: Handle<Node>,
    rigid_body: Handle<Node>,
    pub collider: Handle<Node>,
    shot_timer: f32,
    recoil_offset: Vector3<f32>,
    recoil_target_offset: Vector3<f32>,
    pub index: u32,
    pub controller: PlayerController,
    third_person_model: Handle<Node>,
    first_person_model: Handle<Node>,
    firing_sound_buffer: Option<SoundBufferResource>,
    pub flight_fuel: u32,
    current_player: bool,
    pub ammo: u32,
    first_person_animation_machine: PlayerAnimationMachine,
    third_person_animation_machine: PlayerAnimationMachine,
}

#[derive(Default, Debug)]
pub struct PlayerState {
    pub timestamp: f32,
    pub position: Vector3<f32>,
    pub velocity: Vector3<f32>,
    pub yaw: f32,
    pub pitch: f32,
    pub shoot: bool,
    pub fuel: u32,
}

// impl Serialize for PlayerState {
//     fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
//     where
//         S: Serializer,
//     {
//         let mut state = serializer.serialize_struct("PlayerState", 5)?;
//         state.serialize_field("timestamp", &self.timestamp)?;
//         state.serialize_field("position", &self.position)?;
//         state.serialize_field("velocity", &self.velocity)?;
//         state.serialize_field("yaw", &self.yaw)?;
//         state.serialize_field("pitch", &self.yaw)?;

//         state.end()
//     }
// }

impl Player {
    pub async fn new(
        scene: &mut Scene,
        state: PlayerState,
        resource_manager: ResourceManager,
        current_player: bool,
        index: u32,
    ) -> Self {
        // TODO: Resources should only need to be loaded once and shared among players
        let first_person_resource = resource_manager
            .request_model("data/models/walking_1st.fbx")
            .await
            .unwrap();

        let third_person_resource = resource_manager
            .request_model("data/models/idle.fbx")
            .await
            .unwrap();

        let first_person_model = first_person_resource.instantiate(scene).root;
        let third_person_model = third_person_resource.instantiate(scene).root;

        // let animation_resource = resource_manager
        //     .request_model("data/models/walking_1st.fbx")
        //     .await
        //     .unwrap();

        // let player_model = match current_player {
        //     true => first_person_model,
        //     false => third_person_model,
        // };

        // let animation = *animation_resource
        //     .retarget_animations(player_model, scene)
        //     .get(0)
        //     .unwrap();
        // println!("animations: {:?}", animations.len());

        let camera_pos = Vector3::new(0.0, 0.37, 0.00);
        let model_pos = Vector3::new(0.0, -0.82, -0.09);

        scene.graph[first_person_model]
            .local_transform_mut()
            .set_position(model_pos)
            .set_scale(Vector3::new(0.1, 0.1, 0.1));

        scene.graph[third_person_model]
            .local_transform_mut()
            .set_position(model_pos + camera_pos)
            .set_scale(Vector3::new(0.1, 0.1, 0.1));

        // Show models for first person or third person
        scene.graph[third_person_model].set_visibility(!current_player);
        scene.graph[first_person_model].set_visibility(current_player);

        // Workaround for gun getting culled
        let gun = scene.graph.find_by_name(first_person_model, "gun_LOD0");
        scene.graph[gun]
            .local_transform_mut()
            .set_position(Vector3::new(0.0, 1.0, 0.5));

        let spine = scene.graph.find_by_name(third_person_model, "Bind_Spine");

        // TODO: Need separate pivots for third or first person to make shots appear from correct position in third person
        let barrel = scene.graph.find_by_name(first_person_model, "gun_LOD0");

        let camera = if current_player {
            CameraBuilder::new(
                BaseBuilder::new()
                    .with_children(&[
                        first_person_model,
                        ListenerBuilder::new(BaseBuilder::new()).build(&mut scene.graph),
                    ])
                    .with_local_transform(
                        TransformBuilder::new()
                            .with_local_position(camera_pos)
                            .build(),
                    ),
            )
            .enabled(current_player)
            .with_skybox(create_skybox(resource_manager.clone()).await)
            .build(&mut scene.graph)
        } else {
            CameraBuilder::new(
                BaseBuilder::new()
                    .with_children(&[first_person_model])
                    .with_local_transform(
                        TransformBuilder::new()
                            .with_local_position(camera_pos)
                            .build(),
                    ),
            )
            .enabled(current_player)
            .with_skybox(create_skybox(resource_manager.clone()).await)
            .build(&mut scene.graph)
        };

        scene.graph[camera]
            .as_camera_mut()
            .set_exposure(Exposure::Manual(std::f32::consts::E));

        // let pivot = BaseBuilder::new()
        //     .with_children(&[camera, third_person_model])
        //     .with_tag("player".to_string()) // TODO: Use collider groups instead
        //     .build(&mut scene.graph);

        // Create rigid body, it will be used for interaction with the world.
        // let rigid_body =
        //     RigidBodyBuilder::new_dynamic()
        //         .lock_rotations() // We don't want a bot to tilt.
        //         .translation(Vector3::new(
        //             state.position.x,
        //             state.position.y,
        //             state.position.z,
        //         )) // Set desired position.
        //         .linvel(Vector3::new(
        //             state.velocity.x,
        //             state.velocity.y,
        //             state.velocity.z,
        //         ))
        //         .gravity_scale(GRAVITY_SCALE)
        //         .build(),
        // );

        // Add capsule collider for the rigid body.
        // let collider = scene.physics.add_collider(
        //     ColliderBuilder::capsule_y(0.25, 0.20)
        //         .friction_combine_rule(CoefficientCombineRule::Min)
        //         .friction(0.0)
        //         .build(),
        //     &rigid_body,
        // );

        let collider = ColliderBuilder::new(BaseBuilder::new())
            .with_shape(ColliderShape::capsule_y(0.25, 0.20))
            .with_friction_combine_rule(CoefficientCombineRule::Min)
            .with_friction(0.0)
            .build(&mut scene.graph);

        let rigid_body = RigidBodyBuilder::new(
            BaseBuilder::new()
                .with_tag("player".to_string())
                .with_local_transform(
                    TransformBuilder::new()
                        .with_local_position(Vector3::new(
                            state.position.x,
                            state.position.y,
                            state.position.z,
                        ))
                        .build(),
                )
                .with_children(&[camera, collider, third_person_model]),
        )
        .with_mass(0.0)
        .with_lin_vel(Vector3::new(
            state.velocity.x,
            state.velocity.y,
            state.velocity.z,
        ))
        .with_gravity_scale(GRAVITY_SCALE)
        // We don't want the player to tilt.
        .with_locked_rotations(true)
        // We don't want the rigid body to sleep (be excluded from simulation)
        .with_can_sleep(false)
        .build(&mut scene.graph);

        let firing_sound_buffer = Some(
            resource_manager
                .request_sound_buffer("data/sounds/laser4.ogg")
                .await
                .unwrap(),
        );

        let first_person_animation_machine =
            PlayerAnimationMachine::new(scene, first_person_model, resource_manager.clone()).await;

        let third_person_animation_machine =
            PlayerAnimationMachine::new(scene, third_person_model, resource_manager.clone()).await;

        Self {
            barrel,
            spine,
            camera: camera,
            rigid_body,
            collider,
            shot_timer: 0.0,
            recoil_offset: Default::default(),
            recoil_target_offset: Default::default(),
            index,
            controller: PlayerController {
                shoot: state.shoot,
                yaw: state.yaw,
                pitch: state.pitch,
                ..Default::default()
            },
            first_person_model,
            third_person_model,
            firing_sound_buffer,
            flight_fuel: MAX_FUEL,
            current_player,
            ammo: 20,
            first_person_animation_machine,
            third_person_animation_machine,
        }
    }

    pub fn set_camera(&self, scene: &mut Scene, enabled: bool) {
        if enabled {
            let listener = ListenerBuilder::new(BaseBuilder::new()).build(&mut scene.graph);
            scene.graph.link_nodes(listener, self.camera);
        }

        scene.graph[self.camera]
            .as_camera_mut()
            .set_enabled(enabled);

        scene.graph[self.third_person_model].set_visibility(!enabled);
        scene.graph[self.first_person_model].set_visibility(enabled);
    }

    pub fn update(
        &mut self,
        dt: f32,
        engine: &mut GameEngine,
        scene: Handle<Scene>,
        resource_manager: ResourceManager,
        network_manager: &mut NetworkManager,
        event_sender: &Sender<PlayerEvent>,
        interface: &Interface, // client_address: &mut String,
                               // action_sender: &mpsc::Sender<PlayerEvent>
    ) {
        let scene = &mut engine.scenes[scene];

        self.shot_timer = (self.shot_timer - dt).max(0.0);

        let has_ground_contact = self.has_ground_contact(scene);

        let mut animation_input: PlayerAnimationMachineInput = PlayerAnimationMachineInput {
            on_ground: has_ground_contact,
            walk_forward: self.controller.move_forward,
            ..Default::default()
        };

        // Borrow rigid body in the physics.
        let body = scene.graph[self.rigid_body].as_rigid_body_mut();

        #[cfg(not(feature = "server"))]
        self.interpolate_state(body, dt);

        // Keep only vertical velocity, and drop horizontal.
        let mut velocity = Vector3::new(0.0, body.lin_vel().y, 0.0);

        // TODO: Moving diagonally should move at correct speed

        // Change the velocity depending on the keys pressed.
        if self.controller.move_forward {
            // If we moving forward then add "look" vector of the pivot.
            velocity += body.look_vector().normalize() * MOVEMENT_SPEED;
        }
        if self.controller.move_backward {
            // If we moving backward then subtract "look" vector of the pivot.
            velocity -= body.look_vector().normalize() * MOVEMENT_SPEED;
        }
        if self.controller.move_left {
            // If we moving left then add "side" vector of the pivot.
            velocity += body.side_vector().normalize() * MOVEMENT_SPEED;
        }
        if self.controller.move_right {
            // If we moving right then subtract "side" vector of the pivot.
            velocity -= body.side_vector().normalize() * MOVEMENT_SPEED;
        }

        // Finally new linear velocity.
        body.set_lin_vel(velocity);

        if self.controller.fly && self.has_fuel() {
            if body.lin_vel().y < 3.0 {
                body.apply_impulse(body.up_vector().normalize() * JET_SPEED);
                self.flight_fuel = (self.flight_fuel - 3).clamp(0, MAX_FUEL);
            }

            animation_input.fly = true;
        }

        self.flight_fuel = (self.flight_fuel + 1).clamp(0, MAX_FUEL);

        if self.controller.jump && has_ground_contact && self.can_jump() {
            // TODO: Add "ready_to_jump" for cooldown

            let event = PlayerEvent::Jump { index: self.index };
            let message = NetworkMessage::PlayerEvent {
                index: self.index,
                event,
            };
            #[cfg(feature = "server")]
            network_manager.send_to_all_reliably(&message);

            body.apply_impulse(body.up_vector().normalize() * JUMP_SCALAR);

            animation_input.jump = true;
            scene
                .animations
                .get_mut(self.first_person_animation_machine.jump_animation)
                .set_enabled(true)
                .rewind();
            scene
                .animations
                .get_mut(self.third_person_animation_machine.jump_animation)
                .set_enabled(true)
                .rewind();
        }

        self.controller.jump = false;

        // else if self.has_fuel() {
        //         #[cfg(feature = "server")]
        //         {
        //             self.controller.fly = true;

        //             let event = PlayerEvent::Fly {
        //                 index: self.index,
        //                 active: true,
        //                 fuel: self.flight_fuel,
        //             };
        //             let message = NetworkMessage::PlayerEvent {
        //                 index: self.index,
        //                 event,
        //             };

        //             #[cfg(feature = "server")]
        //             network_manager.send_to_all_reliably(&message);
        //         }
        //     }

        // #[cfg(feature = "server")]
        // if !self.controller.fly {
        //     self.controller.fly = false;

        //     let event = PlayerEvent::Fly {
        //         index: self.index,
        //         active: false,
        //         fuel: self.flight_fuel,
        //     };
        //     let message = NetworkMessage::PlayerEvent {
        //         index: self.index,
        //         event,
        //     };

        //     network_manager.send_to_all_reliably(&message);
        // }

        // Change the rotation of the rigid body according to current yaw. These lines responsible for
        // left-right rotation.
        // let mut position = *body.position();
        // position.rotation =
        //     UnitQuaternion::from_axis_angle(&Vector3::y_axis(), self.controller.yaw.to_radians());

        body.local_transform_mut()
            .set_rotation(UnitQuaternion::from_axis_angle(
                &Vector3::y_axis(),
                self.controller.yaw.to_radians(),
            ));

        // Set pitch for the camera. These lines responsible for up-down camera rotation.
        scene.graph[self.camera].local_transform_mut().set_rotation(
            UnitQuaternion::from_axis_angle(&Vector3::x_axis(), self.controller.pitch.to_radians()),
        );

        scene.graph[self.spine].local_transform_mut().set_rotation(
            UnitQuaternion::from_axis_angle(&Vector3::x_axis(), self.controller.pitch.to_radians()),
        );

        if self.controller.shoot {
            // TODO: Ammo check here
            self.shoot_weapon(scene, resource_manager, network_manager, &event_sender);
            animation_input.shoot = true;
        }

        // Update listener position if camera is active
        // let camera = &scene.graph[self.camera];
        // if camera.as_camera().is_enabled() {
        //     let mut ctx = scene.graph.sound_context.state();
        //     let listener = ctx.listener_mut();
        //     let listener_basis = Matrix3::from_columns(&[
        //         camera.side_vector(),
        //         camera.up_vector(),
        //         -camera.look_vector(),
        //     ]);
        //     listener.set_position(camera.global_position());
        //     listener.set_basis(listener_basis);
        // }

        #[cfg(feature = "server")]
        if scene.graph[self.rigid_body].global_position().y < -12.0 {
            event_sender
                .send(PlayerEvent::KillPlayerFromIntersection {
                    collider: self.collider,
                })
                .unwrap();
        }

        if self.current_player {
            engine.user_interface.send_message(TextMessage::text(
                interface.fuel,
                MessageDirection::ToWidget,
                format!("{} / {}", self.flight_fuel, MAX_FUEL),
            ));
        }

        self.first_person_animation_machine
            .update(scene, dt, animation_input);
        self.third_person_animation_machine
            .update(scene, dt, animation_input);
    }

    fn can_jump(&self) -> bool {
        // TODO: Add cooldown timer and test for ground contact
        return true;
    }

    #[cfg(not(feature = "server"))]
    fn interpolate_state(&mut self, body: &mut RigidBody, dt: f32) {
        // if length > buffer_length {
        //     self.controller
        //         .previous_states
        //         .drain(0..length - buffer_length + 1);
        // }
        if let Some(new_state) = &self.controller.new_states.first_mut() {
            // self.controller.new_state = None;
            if let Some(previous_state) = self.controller.previous_states.first_mut() {
                // Only sync vertical velocity
                // let mut velocity_diff: Vector3<f32> =
                //     Vector3::new(0.0, new_state.velocity.y - previous_state.velocity.y, 0.0);
                // let velocity_diff_mag = velocity_diff.magnitude();

                // if velocity_diff_mag > 0.0 {
                //     let max_change = 9.8 * GRAVITY_SCALE * dt / 6.0 as f32;
                //     let velocity_change = f32::min(velocity_diff_mag, max_change);
                //     velocity_diff *= velocity_change / velocity_diff_mag;
                //     previous_state.velocity += velocity_diff;

                //     let new_velocity = *body.lin_vel() + velocity_diff;
                //     body.set_lin_vel(new_velocity, true);
                // }

                // Sync position
                let mut pos_diff: Vector3<f32> = new_state.position - previous_state.position;
                let pos_diff_mag = pos_diff.magnitude();

                if pos_diff_mag > f32::EPSILON {
                    let min_smooth_speed: f32 = MOVEMENT_SPEED / 6.0;
                    let target_catchup_time: f32 = 0.15;

                    self.controller.smoothing_speed = f32::max(
                        self.controller.smoothing_speed,
                        f32::max(min_smooth_speed, pos_diff_mag / target_catchup_time),
                    );

                    let max_move = dt * self.controller.smoothing_speed;

                    // let max_tolerated_distance = MOVEMENT_SPEED * dt * 3.0;
                    // let min_move = MOVEMENT_SPEED * dt / 8.0;
                    // let max_move =
                    //     f32::max(min_move, (pos_diff_mag - max_tolerated_distance) / 6.0);

                    let move_dist = f32::min(pos_diff_mag, max_move);
                    pos_diff *= move_dist / pos_diff_mag;

                    // let new_pos = Translation3::from(pos_diff) * (*body.global_position());
                    body.local_transform_mut().offset(pos_diff);

                    for previous_state in self.controller.previous_states.iter_mut() {
                        previous_state.position += pos_diff;
                    }

                    if (move_dist - pos_diff_mag).abs() < f32::EPSILON {
                        self.controller.smoothing_speed = 0.0;
                        self.controller.new_states.remove(0);
                    }
                } else {
                    self.controller.smoothing_speed = 0.0;
                    // self.controller
                    //     .previous_states
                    //     .remove(SYNC_FREQUENCY as usize);
                    self.controller.new_states.remove(0);
                }
            }
        }
    }

    pub fn has_fuel(&self) -> bool {
        self.flight_fuel >= 3
    }

    pub fn can_shoot(&self) -> bool {
        self.shot_timer <= 0.0
    }

    fn play_shoot_sound(&self, scene: &mut Scene) {
        let source = SoundBuilder::new(
            BaseBuilder::new().with_local_transform(
                TransformBuilder::new()
                    .with_local_position(scene.graph[self.barrel].global_position())
                    .build(),
            ),
        )
        .with_play_once(true)
        .with_buffer(self.firing_sound_buffer.clone())
        .with_radius(1.0)
        .with_status(Status::Playing)
        .build(&mut scene.graph);
        // let mut ctx = scene.sound_context.state();
        // ctx.add_source(
        //     SpatialSourceBuilder::new(
        //         GenericSourceBuilder::new()
        //             .with_buffer(self.firing_sound_buffer.clone().into())
        //             .with_play_once(true)
        //             // Every sound source must be explicity set to Playing status, otherwise it will be stopped.
        //             .with_status(Status::Playing)
        //             .build()
        //             .unwrap(),
        //     )
        //     .with_position(scene.graph[self.barrel].global_position())
        //     // .with_rolloff_factor(1.5)
        //     .build_source(),
        // );
    }

    fn shoot_weapon(
        &mut self,
        scene: &mut Scene,
        resource_manager: ResourceManager,
        network_manager: &mut NetworkManager,
        event_sender: &Sender<PlayerEvent>,
    ) {
        if self.can_shoot() {
            self.shot_timer = 0.1;

            // self.recoil_target_offset = Vector3::new(0.0, 0.0, -0.035);

            let mut intersections = Vec::new();

            // TODO: Need to use a third person weapon pivot if camera is not enabled

            // Make a ray that starts at the weapon's position in the world and look toward
            // "look" vector of the camera.
            let ray = Ray::new(
                scene.graph[self.camera].global_position(),
                scene.graph[self.camera]
                    .look_vector()
                    .normalize()
                    .scale(1000.0),
            );

            scene.graph.physics.cast_ray(
                RayCastOptions {
                    ray_origin: ray.origin.into(),
                    ray_direction: ray.dir,
                    max_len: ray.dir.norm(),
                    groups: Default::default(),
                    sort_results: true, // We need intersections to be sorted from closest to furthest.
                },
                &mut intersections,
            );

            // Ignore intersections with player's capsule.
            let trail_length = if let Some(intersection) =
                intersections.iter().find(|i| i.collider != self.collider)
            {
                let node_handle = scene.graph[intersection.collider].parent();
                let node = &mut scene.graph[node_handle];
                if node.is_rigid_body() {
                    let tag = node.tag();

                    #[cfg(feature = "server")]
                    let mut destroy_block = false;
                    #[cfg(feature = "server")]
                    let mut kill_player = false;

                    // TODO: Should probably use collider groups instead of tag?
                    match tag {
                        "wall" => (),
                        "player" => {
                            #[cfg(feature = "server")]
                            node.set_tag("player_1_hp".to_string());
                        }
                        #[cfg(feature = "server")]
                        "player_1_hp" => {
                            kill_player = true;
                        }
                        #[cfg(feature = "server")]
                        "destructable" => {
                            destroy_block = true;
                        }
                        _ => {
                            #[cfg(feature = "server")]
                            node.set_tag("destructable".to_string());
                        }
                    }

                    #[cfg(feature = "server")]
                    if destroy_block {
                        let event = PlayerEvent::DestroyBlock {
                            index: node_handle.index(),
                        };
                        let message = NetworkMessage::PlayerEvent {
                            index: node_handle.index(),
                            event: event,
                        };

                        // network_manager.send_to_all_unreliably(&message, 2);
                        network_manager.send_to_all_reliably(&message);
                        event_sender.send(event).unwrap();
                    }

                    #[cfg(feature = "server")]
                    if kill_player {
                        let event = PlayerEvent::KillPlayerFromIntersection {
                            collider: intersection.collider,
                        };
                        event_sender.send(event).unwrap();
                    }
                }

                // Add bullet impact effect.
                // let effect_orientation = if intersection.normal.normalize() == Vector3::y() {
                //     // Handle singularity when normal of impact point is collinear with Y axis.
                //     UnitQuaternion::from_axis_angle(&Vector3::y_axis(), 0.0)
                // } else {
                //     UnitQuaternion::face_towards(&intersection.normal, &Vector3::y())
                // };

                // create_bullet_impact(
                //     &mut scene.graph,
                //     resource_manager.clone(),
                //     intersection.position.coords,
                //     effect_orientation,
                // );

                // Trail length will be the length of line between intersection point and ray origin.
                (intersection.position.coords - ray.origin).norm()
            } else {
                // Otherwise trail length will be just the ray length.
                ray.dir.norm()
            };

            // #[cfg(not(feature = "server"))]
            // create_shot_trail(&mut scene.graph, ray.origin, ray.dir, trail_length);

            #[cfg(not(feature = "server"))]
            self.play_shoot_sound(scene);

            // Reset camera rotation
            // scene.graph[self.camera]
            //     .local_transform_mut()
            //     .set_rotation(original_rotation);
        }
    }

    pub fn get_velocity(&self, scene: &Scene) -> Vector3<f32> {
        let body = scene.graph[self.rigid_body].as_rigid_body();

        body.lin_vel()
    }

    pub fn get_position(&self, scene: &Scene) -> Vector3<f32> {
        let body = &scene.graph[self.rigid_body];

        body.global_position()
    }

    pub fn get_yaw(&self) -> f32 {
        self.controller.yaw
    }

    pub fn get_pitch(&self) -> f32 {
        self.controller.pitch
    }

    pub fn clean_up(&mut self, scene: &mut Scene) {
        scene.remove_node(self.rigid_body);
    }

    pub fn has_ground_contact(&self, scene: &Scene) -> bool {
        let graph = &scene.graph;
        if let Some(Node::Collider(collider)) = graph.try_get(self.collider) {
            for contact in collider.contacts(&graph.physics) {
                for manifold in contact.manifolds.iter() {
                    if manifold.local_n1.y.abs() > 0.7 || manifold.local_n2.y.abs() > 0.7 {
                        return true;
                    }
                }
            }
        }
        false
    }
}

async fn create_skybox(resource_manager: ResourceManager) -> SkyBox {
    // Load skybox textures in parallel.
    let (front, back, left, right, top, bottom) = fyrox::core::futures::join!(
        resource_manager.request_texture("data/textures/skybox/front.png"),
        resource_manager.request_texture("data/textures/skybox/back.png"),
        resource_manager.request_texture("data/textures/skybox/left.png"),
        resource_manager.request_texture("data/textures/skybox/right.png"),
        resource_manager.request_texture("data/textures/skybox/top.png"),
        resource_manager.request_texture("data/textures/skybox/down.png")
    );

    // Unwrap everything.
    let skybox = SkyBoxBuilder {
        front: Some(front.unwrap()),
        back: Some(back.unwrap()),
        left: Some(left.unwrap()),
        right: Some(right.unwrap()),
        top: Some(top.unwrap()),
        bottom: Some(bottom.unwrap()),
    }
    .build()
    .unwrap();

    // Set S and T coordinate wrap mode, ClampToEdge will remove any possible seams on edges
    // of the skybox.
    let cubemap = skybox.cubemap();
    let mut data = cubemap.as_ref().unwrap().data_ref();
    data.set_s_wrap_mode(TextureWrapMode::ClampToEdge);
    data.set_t_wrap_mode(TextureWrapMode::ClampToEdge);

    skybox
}

// #[cfg(not(feature = "server"))]
// fn create_bullet_impact(
//     graph: &mut Graph,
//     resource_manager: ResourceManager,
//     pos: Vector3<f32>,
//     orientation: UnitQuaternion<f32>,
// ) -> Handle<Node> {
//     // Create sphere emitter first.
//     let emitter = SphereEmitterBuilder::new(
//         BaseEmitterBuilder::new()
//             .with_max_particles(200)
//             .with_spawn_rate(1000)
//             .with_size_modifier_range(NumericRange::new(-0.01, -0.0125))
//             .with_size_range(NumericRange::new(0.0010, 0.01))
//             .with_x_velocity_range(NumericRange::new(-0.01, 0.01))
//             .with_y_velocity_range(NumericRange::new(0.017, 0.02))
//             .with_z_velocity_range(NumericRange::new(-0.01, 0.01))
//             .resurrect_particles(false),
//     )
//     .with_radius(0.01)
//     .build();

//     // Color gradient will be used to modify color of each particle over its lifetime.
//     let color_gradient = {
//         let mut gradient = ColorGradient::new();
//         gradient.add_point(GradientPoint::new(0.00, Color::from_rgba(255, 255, 0, 0)));
//         gradient.add_point(GradientPoint::new(0.05, Color::from_rgba(255, 160, 0, 255)));
//         gradient.add_point(GradientPoint::new(0.95, Color::from_rgba(255, 120, 0, 255)));
//         gradient.add_point(GradientPoint::new(1.00, Color::from_rgba(255, 60, 0, 0)));
//         gradient
//     };

//     // Create new transform to orient and position particle system.
//     let transform = TransformBuilder::new()
//         .with_local_position(pos)
//         .with_local_rotation(orientation)
//         .build();

//     // Finally create particle system with limited lifetime.
//     ParticleSystemBuilder::new(
//         BaseBuilder::new()
//             .with_lifetime(1.0)
//             .with_local_transform(transform),
//     )
//     .with_acceleration(Vector3::new(0.0, -10.0, 0.0))
//     .with_color_over_lifetime_gradient(color_gradient)
//     .with_emitters(vec![emitter])
//     // We'll use simple spark texture for each particle.
//     .with_texture(resource_manager.request_texture(Path::new("data/textures/spark.png")))
//     .build(graph)
// }

#[cfg(not(feature = "server"))]
fn create_shot_trail(
    graph: &mut Graph,
    origin: Vector3<f32>,
    direction: Vector3<f32>,
    trail_length: f32,
) {
    use std::sync::Arc;

    use fyrox::core::{parking_lot::Mutex, sstorage::ImmutableString};

    let transform = TransformBuilder::new()
        .with_local_position(origin)
        // Scale the trail in XZ plane to make it thin, and apply `trail_length` scale on Y axis
        // to stretch is out.
        .with_local_scale(Vector3::new(0.008, 0.008, trail_length))
        // Rotate the trail along given `direction`
        .with_local_rotation(UnitQuaternion::face_towards(&direction, &Vector3::y()))
        .build();

    // Create unit cylinder with caps that faces toward Z axis.
    let shape = Arc::new(Mutex::new(SurfaceData::make_cylinder(
        6,    // Count of sides
        0.5,  // Radius
        1.0,  // Height
        true, // No caps are needed.
        // Rotate vertical cylinder around X axis to make it face towards Z axis
        &UnitQuaternion::from_axis_angle(&Vector3::x_axis(), 90.0f32.to_radians()).to_homogeneous(),
    )));
    let mut material = Material::standard();
    material
        .set_property(
            &ImmutableString::new("diffuseColor"),
            PropertyValue::Color(Color::from_rgba(105, 171, 195, 150)),
        )
        .unwrap();

    MeshBuilder::new(
        BaseBuilder::new()
            .with_local_transform(transform)
            .with_lifetime(0.05),
    )
    .with_surfaces(vec![SurfaceBuilder::new(shape)
        .with_material(Arc::new(Mutex::new(material)))
        .build()])
    // Do not cast shadows.
    .with_cast_shadows(false)
    // Make sure to set Forward render path, otherwise the object won't be
    // transparent.
    .with_render_path(RenderPath::Forward)
    .build(graph);
}

fn lerp(a: f32, b: f32, f: f32) -> f32 {
    return (a * (1.0 - f)) + (b * f);
}

fn get_jump_impulse(dist: f32, g: f32, mass: f32) -> f32 {
    let v = (2.0 * g * dist).sqrt();

    mass * v
}
