use bincode::serialize;
use rg3d::{
    core::{
        algebra::{Matrix3, Translation3, UnitQuaternion, Vector3},
        color::Color,
        color_gradient::{ColorGradient, GradientPoint},
        math::{ray::Ray, Vector3Ext},
        numeric_range::NumericRange,
        pool::Handle,
    },
    engine::{
        resource_manager::{MaterialSearchOptions, ResourceManager},
        ColliderHandle, RigidBodyHandle,
    },
    event::ElementState,
    gui::message::{MessageDirection, TextMessage},
    material::{Material, PropertyValue},
    physics::{
        dynamics::{RigidBody, RigidBodyBuilder},
        geometry::ColliderBuilder,
        prelude::CoefficientCombineRule,
    },
    resource::texture::TextureWrapMode,
    scene::{
        base::BaseBuilder,
        camera::{CameraBuilder, SkyBox, SkyBoxBuilder},
        graph::Graph,
        mesh::{
            surface::{SurfaceBuilder, SurfaceData},
            MeshBuilder, RenderPath,
        },
        node::Node,
        particle_system::ParticleSystemBuilder,
        physics::RayCastOptions,
        transform::TransformBuilder,
        Scene,
    },
    sound::{
        buffer::SoundBufferResource,
        source::{generic::GenericSourceBuilder, spatial::SpatialSourceBuilder, Status},
    },
};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Sender},
        Arc, Mutex, RwLock,
    },
};

use crate::{
    level::Level,
    network_manager::{self, NetworkManager, NetworkMessage},
    player_event::PlayerEvent,
    GameEngine, Interface,
};

const MOVEMENT_SPEED: f32 = 1.5;
const GRAVITY_SCALE: f32 = 0.5;
const JET_SPEED: f32 = 0.012;

const MAX_FUEL: u32 = 200;
pub const SYNC_FREQUENCY: u32 = 3;

#[derive(Default)]
pub struct PlayerController {
    pub move_forward: bool,
    pub move_backward: bool,
    pub move_left: bool,
    pub move_right: bool,
    pub move_up: bool,
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
    pivot: Handle<Node>,
    weapon_pivot: Handle<Node>,
    spine: Handle<Node>,
    camera: Handle<Node>,
    rigid_body: RigidBodyHandle,
    pub collider: ColliderHandle,
    shot_timer: f32,
    recoil_offset: Vector3<f32>,
    recoil_target_offset: Vector3<f32>,
    pub index: u32,
    pub controller: PlayerController,
    third_person_model: Handle<Node>,
    first_person_model: Handle<Node>,
    firing_sound_buffer: SoundBufferResource,
    pub flight_fuel: u32,
    current_player: bool,
}

#[derive(Default, Debug)]
pub struct PlayerState {
    pub timestamp: f32,
    pub position: Vector3<f32>,
    pub velocity: Vector3<f32>,
    pub yaw: f32,
    pub pitch: f32,
    pub shoot: bool,
    // pub fuel: u32,
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
        let model_resource = resource_manager
            .request_model(
                "data/models/shooting.rgs",
                MaterialSearchOptions::MaterialsDirectory(PathBuf::from("data/textures")),
            )
            .await
            .unwrap();

        let first_person_model = model_resource.instantiate(scene).root;

        let third_person_model = model_resource.instantiate(scene).root;

        let camera_pos = Vector3::new(0.0, 0.37, 0.00);
        let model_pos = Vector3::new(0.0, -0.82, -0.08);

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
        // let gun = scene.graph.find_by_name(first_person_model, "gun_LOD0");
        // scene.graph[gun]
        //     .local_transform_mut()
        //     .set_position(Vector3::new(0.0, 0.82, 0.5));

        let spine = scene.graph.find_by_name(third_person_model, "Bind_Spine");

        // TODO: Need separate pivots for third or first person to make shots appear from correct position in third person
        let weapon_pivot = scene.graph.find_by_name(first_person_model, "Barrel");

        let camera = CameraBuilder::new(
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
        .build(&mut scene.graph);

        let pivot = BaseBuilder::new()
            .with_children(&[camera, third_person_model])
            .with_tag("player".to_string()) // TODO: Use collider groups instead
            .build(&mut scene.graph);

        // Create rigid body, it will be used for interaction with the world.
        let rigid_body = scene.physics.add_body(
            RigidBodyBuilder::new_dynamic()
                .lock_rotations() // We don't want a bot to tilt.
                .translation(Vector3::new(
                    state.position.x,
                    state.position.y,
                    state.position.z,
                )) // Set desired position.
                .linvel(Vector3::new(
                    state.velocity.x,
                    state.velocity.y,
                    state.velocity.z,
                ))
                .gravity_scale(GRAVITY_SCALE)
                .build(),
        );

        // Add capsule collider for the rigid body.
        let collider = scene.physics.add_collider(
            ColliderBuilder::capsule_y(0.25, 0.20)
                .friction_combine_rule(CoefficientCombineRule::Min)
                .friction(0.0)
                .build(),
            &rigid_body,
        );

        // Bind pivot with rigid body. Scene will automatically sync transform of the pivot
        // with the transform of the rigid body.
        scene.physics_binder.bind(pivot, rigid_body);

        let firing_sound_buffer = resource_manager
            .request_sound_buffer("data/sounds/laser4.ogg", false)
            .await
            .unwrap();

        Self {
            pivot,
            weapon_pivot,
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
        }
    }

    pub fn set_camera(&self, scene: &mut Scene, enabled: bool) {
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

        // `follow` method defined in Vector3Ext trait and it just increases or
        // decreases vector's value in order to "follow" the target value with
        // given speed.
        self.recoil_offset.follow(&self.recoil_target_offset, 0.5);

        // Apply offset to weapon's model.
        // scene.graph[self.weapon_pivot]
        //     .local_transform_mut()
        //     .set_position(self.recoil_offset);

        // Check if we've reached target recoil offset.
        if self
            .recoil_offset
            .metric_distance(&self.recoil_target_offset)
            < 0.001
        {
            // And if so, reset offset to zero to return weapon at
            // its default position.
            self.recoil_target_offset = Default::default();
        }

        // Borrow rigid body in the physics.
        let body = scene.physics.bodies.get_mut(&self.rigid_body).unwrap();

        #[cfg(not(feature = "server"))]
        self.interpolate_state(body, dt);

        let pivot = &mut scene.graph[self.pivot];

        // Keep only vertical velocity, and drop horizontal.
        let mut velocity = Vector3::new(0.0, body.linvel().y, 0.0);

        // Change the velocity depending on the keys pressed.
        if self.controller.move_forward {
            // If we moving forward then add "look" vector of the pivot.
            velocity += pivot.look_vector() * MOVEMENT_SPEED;
        }
        if self.controller.move_backward {
            // If we moving backward then subtract "look" vector of the pivot.
            velocity -= pivot.look_vector() * MOVEMENT_SPEED;
        }
        if self.controller.move_left {
            // If we moving left then add "side" vector of the pivot.
            velocity += pivot.side_vector() * MOVEMENT_SPEED;
        }
        if self.controller.move_right {
            // If we moving right then subtract "side" vector of the pivot.
            velocity -= pivot.side_vector() * MOVEMENT_SPEED;
        }

        // Finally new linear velocity.
        body.set_linvel(velocity, true);

        if self.controller.move_up && self.can_fly() {
            body.apply_impulse(pivot.up_vector() * JET_SPEED, true);
            self.flight_fuel = (self.flight_fuel - 2).clamp(0, 200);
        } else {
            self.flight_fuel = (self.flight_fuel + 1).clamp(0, 200);
        }

        // Change the rotation of the rigid body according to current yaw. These lines responsible for
        // left-right rotation.
        let mut position = *body.position();
        position.rotation =
            UnitQuaternion::from_axis_angle(&Vector3::y_axis(), self.controller.yaw.to_radians());

        body.set_position(position, true);

        // Set pitch for the camera. These lines responsible for up-down camera rotation.
        scene.graph[self.camera].local_transform_mut().set_rotation(
            UnitQuaternion::from_axis_angle(&Vector3::x_axis(), self.controller.pitch.to_radians()),
        );

        scene.graph[self.spine].local_transform_mut().set_rotation(
            UnitQuaternion::from_axis_angle(&Vector3::x_axis(), self.controller.pitch.to_radians()),
        );

        if self.controller.shoot {
            self.shoot_weapon(scene, resource_manager, network_manager, &event_sender);
        }

        // Update listener position if camera is active
        let camera = &scene.graph[self.camera];
        if camera.as_camera().is_enabled() {
            let mut ctx = scene.sound_context.state();
            let listener = ctx.listener_mut();
            let listener_basis = Matrix3::from_columns(&[
                camera.side_vector(),
                camera.up_vector(),
                -camera.look_vector(),
            ]);
            listener.set_position(camera.global_position());
            listener.set_basis(listener_basis);
        }

        #[cfg(feature = "server")]
        if position.translation.vector.y < -10.0 {
            event_sender
                .send(PlayerEvent::KillPlayerFromIntersection {
                    collider: self.collider,
                })
                .unwrap();
        }

        engine.user_interface.send_message(TextMessage::text(
            interface.fuel,
            MessageDirection::ToWidget,
            format!("{} / {}", self.flight_fuel, MAX_FUEL),
        ));
    }

    #[cfg(not(feature = "server"))]
    fn interpolate_state(&mut self, body: &mut RigidBody, dt: f32) {
        // if length > buffer_length {
        //     self.controller
        //         .previous_states
        //         .drain(0..length - buffer_length + 1);
        // }
        if let Some(new_state) = &self.controller.new_states.first_mut() {
            // let minimum_distance = self
            //     .controller
            //     .previous_states
            //     .iter_mut()
            //     .enumerate()
            //     .map(|(index, previous_state)| {
            //         (
            //             index,
            //             (new_state.position - previous_state.position).magnitude() as f32,
            //         )
            //     })
            //     .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            // println!("minimum distance: {:?}", minimum_distance);
            // println!("length: {}", self.controller.previous_states.len());
            // body.set_position(
            //     Isometry::from_parts(
            //         Translation3::from(new_state.position),
            //         (*body.position()).rotation,
            //     ),
            //     true,
            // );
            // self.controller.new_state = None;
            if let Some(previous_state) = self.controller.previous_states.first_mut() {
                // Only sync vertical velocity
                let mut velocity_diff: Vector3<f32> =
                    Vector3::new(0.0, new_state.velocity.y - previous_state.velocity.y, 0.0);
                let velocity_diff_mag = velocity_diff.magnitude();

                if velocity_diff_mag > 0.0 {
                    let max_change = 9.8 * GRAVITY_SCALE * dt / 6.0 as f32;
                    let velocity_change = f32::min(velocity_diff_mag, max_change);
                    velocity_diff *= velocity_change / velocity_diff_mag;
                    previous_state.velocity += velocity_diff;

                    let new_velocity = *body.linvel() + velocity_diff;
                    body.set_linvel(new_velocity, true);
                }

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

                    let new_pos = Translation3::from(pos_diff) * (*body.position());
                    body.set_position(new_pos, true);

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

    pub fn can_fly(&self) -> bool {
        self.flight_fuel >= 2
    }

    pub fn can_shoot(&self) -> bool {
        self.shot_timer <= 0.0
    }

    fn play_shoot_sound(&self, scene: &mut Scene) {
        let mut ctx = scene.sound_context.state();
        ctx.add_source(
            SpatialSourceBuilder::new(
                GenericSourceBuilder::new()
                    .with_buffer(self.firing_sound_buffer.clone().into())
                    .with_play_once(true)
                    // Every sound source must be explicity set to Playing status, otherwise it will be stopped.
                    .with_status(Status::Playing)
                    .build()
                    .unwrap(),
            )
            .with_position(scene.graph[self.weapon_pivot].global_position())
            .with_rolloff_factor(1.5)
            .build_source(),
        );
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

            self.recoil_target_offset = Vector3::new(0.0, 0.0, -0.035);

            let mut intersections = Vec::new();

            // TODO: Need to use a third person weapon pivot if camera is not enabled

            // Make a ray that starts at the weapon's position in the world and look toward
            // "look" vector of the camera.
            let ray = Ray::new(
                scene.graph[self.weapon_pivot].global_position(),
                scene.graph[self.camera].look_vector().scale(1000.0),
            );

            scene.physics.cast_ray(
                RayCastOptions {
                    ray,
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
                let body = scene
                    .physics
                    .collider_parent(&intersection.collider)
                    .unwrap();

                if let Some(node_handle) = scene.physics_binder.node_of(*body) {
                    let node = &mut scene.graph[node_handle];
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

            #[cfg(not(feature = "server"))]
            create_shot_trail(&mut scene.graph, ray.origin, ray.dir, trail_length);

            #[cfg(not(feature = "server"))]
            self.play_shoot_sound(scene);

            // Reset camera rotation
            // scene.graph[self.camera]
            //     .local_transform_mut()
            //     .set_rotation(original_rotation);
        }
    }

    pub fn get_velocity(&self, scene: &Scene) -> Vector3<f32> {
        let body = scene.physics.bodies.get(&self.rigid_body).unwrap();

        *body.linvel()
    }

    pub fn get_position(&self, scene: &Scene) -> Vector3<f32> {
        let body = scene.physics.bodies.get(&self.rigid_body).unwrap();

        body.position().translation.vector
    }

    pub fn get_yaw(&self) -> f32 {
        self.controller.yaw
    }

    pub fn get_pitch(&self) -> f32 {
        self.controller.pitch
    }

    pub fn clean_up(&mut self, scene: &mut Scene) {
        scene.physics.remove_body(&self.rigid_body);
        scene.remove_node(self.pivot);
    }
}

async fn create_skybox(resource_manager: ResourceManager) -> SkyBox {
    // Load skybox textures in parallel.
    let (front, back, left, right, top, bottom) = rg3d::core::futures::join!(
        resource_manager.request_texture("data/textures/skybox/front.png", None),
        resource_manager.request_texture("data/textures/skybox/back.png", None),
        resource_manager.request_texture("data/textures/skybox/left.png", None),
        resource_manager.request_texture("data/textures/skybox/right.png", None),
        resource_manager.request_texture("data/textures/skybox/top.png", None),
        resource_manager.request_texture("data/textures/skybox/down.png", None)
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
    let transform = TransformBuilder::new()
        .with_local_position(origin)
        // Scale the trail in XZ plane to make it thin, and apply `trail_length` scale on Y axis
        // to stretch is out.
        .with_local_scale(Vector3::new(0.008, 0.008, trail_length))
        // Rotate the trail along given `direction`
        .with_local_rotation(UnitQuaternion::face_towards(&direction, &Vector3::y()))
        .build();

    // Create unit cylinder with caps that faces toward Z axis.
    let shape = Arc::new(RwLock::new(SurfaceData::make_cylinder(
        6,    // Count of sides
        1.0,  // Radius
        1.0,  // Height
        true, // No caps are needed.
        // Rotate vertical cylinder around X axis to make it face towards Z axis
        &UnitQuaternion::from_axis_angle(&Vector3::x_axis(), 90.0f32.to_radians()).to_homogeneous(),
    )));

    let mut material = Material::standard();
    material
        .set_property(
            "diffuseColor",
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
