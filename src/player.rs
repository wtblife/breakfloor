use bincode::serialize;
use rg3d::{
    core::{
        algebra::{Isometry, Matrix3, Transform3, Translation3, UnitQuaternion, Vector3},
        color::Color,
        color_gradient::{ColorGradient, GradientPoint},
        math::{ray::Ray, Matrix3Ext, Matrix4Ext, Vector3Ext},
        numeric_range::NumericRange,
        pool::Handle,
    },
    engine::resource_manager::{ResourceManager, SharedSoundBuffer},
    event::ElementState,
    physics::{
        dynamics::{RigidBody, RigidBodyBuilder},
        geometry::ColliderBuilder,
    },
    renderer::surface::{SurfaceBuilder, SurfaceSharedData},
    resource::{model, texture::TextureWrapMode},
    scene::{
        base::BaseBuilder,
        camera::{self, CameraBuilder, SkyBox},
        graph::Graph,
        mesh::{MeshBuilder, RenderPath},
        node::Node,
        particle_system::{BaseEmitterBuilder, ParticleSystemBuilder, SphereEmitterBuilder},
        physics::RayCastOptions,
        transform::TransformBuilder,
        ColliderHandle, RigidBodyHandle, Scene,
    },
    sound::{
        buffer::SoundBuffer,
        source::{generic::GenericSourceBuilder, spatial::SpatialSourceBuilder, Status},
    },
};
use std::{
    net::SocketAddr,
    path::Path,
    sync::{
        mpsc::{self, Sender},
        Arc, RwLock,
    },
};

use crate::{
    level::Level,
    network_manager::{self, NetworkManager, NetworkMessage},
    player_event::PlayerEvent,
    GameEngine,
};

const MOVEMENT_SPEED: f32 = 1.5;
const GRAVITY_SCALE: f32 = 0.5;
const JET_SPEED: f32 = 0.012;

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
    pub new_state: Option<PlayerState>,
    pub previous_states: Vec<PlayerState>,
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
    firing_sound_buffer: SharedSoundBuffer,
    pub flight_fuel: u32,
}

#[derive(Default)]
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
        let model_resource = resource_manager
            .request_model("data/models/shooting.rgs")
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
                .translation(state.position.x, state.position.y, state.position.z) // Set desired position.
                .linvel(state.velocity.x, state.velocity.y, state.velocity.z)
                .gravity_scale(GRAVITY_SCALE)
                .build(),
        );

        // Add capsule collider for the rigid body.
        let collider = scene.physics.add_collider(
            ColliderBuilder::capsule_y(0.25, 0.20).friction(0.0).build(),
            rigid_body,
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
            flight_fuel: 200,
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
        scene: &mut Scene,
        resource_manager: ResourceManager,
        network_manager: &mut NetworkManager,
        event_sender: &Sender<PlayerEvent>,
        // client_address: &mut String,
        // action_sender: &mpsc::Sender<PlayerEvent>,
    ) {
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

        let pivot = &mut scene.graph[self.pivot];

        // Borrow rigid body in the physics.
        let body = scene
            .physics
            .bodies
            .get_mut(self.rigid_body.into())
            .unwrap();

        #[cfg(not(feature = "server"))]
        self.interpolate_state(body, dt);

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
    }

    #[cfg(not(feature = "server"))]
    fn interpolate_state(&mut self, body: &mut RigidBody, dt: f32) {
        let mut fuel = self.flight_fuel;
        if let Some(new_state) = self.controller.new_state.take() {
            self.controller.previous_states.retain(|previous_state| {
                let matches = previous_state.timestamp == new_state.timestamp;
                if matches {
                    let distance = new_state.position.sqr_distance(&previous_state.position);
                    let max_distance_tolerated = MOVEMENT_SPEED / 2.0;
                    let correction_percentage = (distance / max_distance_tolerated).clamp(0.0, 1.0);

                    if correction_percentage > f32::EPSILON {
                        let mut pos = *body.position();
                        pos = pos.lerp_slerp(
                            &Isometry::from_parts(
                                Translation3::new(
                                    new_state.position.x,
                                    new_state.position.y,
                                    new_state.position.z,
                                ),
                                pos.rotation,
                            ),
                            correction_percentage,
                        );
                        body.set_position(pos, true);

                        // println!("corrected position by: {}", correction_percentage);
                    }

                    let velocity_difference =
                        (new_state.velocity.y - previous_state.velocity.y).abs();
                    let max_difference_tolerated = 9.8 * GRAVITY_SCALE * 2.0;
                    let velocity_correction =
                        (velocity_difference / max_difference_tolerated).clamp(0.0, 1.0);

                    if velocity_correction > f32::EPSILON {
                        let new_vertical_velocity =
                            lerp(body.linvel().y, new_state.velocity.y, velocity_correction);

                        let velocity = Vector3::new(0.0, new_vertical_velocity, 0.0);

                        body.set_linvel(velocity, true);
                        // println!("corrected velocity by: {}", velocity_correction);
                    }

                    fuel = new_state.fuel;
                }

                !matches
            });
        }

        self.flight_fuel = fuel;
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
                GenericSourceBuilder::new(self.firing_sound_buffer.clone().into())
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
                let collider = scene
                    .physics
                    .colliders
                    .get(intersection.collider.into())
                    .unwrap();

                let body = collider.parent();

                if let Some(node_handle) = scene.physics_binder.node_of(body.into()) {
                    let node = &mut scene.graph[node_handle];
                    let tag = node.tag().clone();

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
        let body = scene.physics.bodies.get(self.rigid_body.into()).unwrap();

        *body.linvel()
    }

    pub fn get_position(&self, scene: &Scene) -> Vector3<f32> {
        let body = scene.physics.bodies.get(self.rigid_body.into()).unwrap();

        body.position().translation.vector
    }

    pub fn get_yaw(&self) -> f32 {
        self.controller.yaw
    }

    pub fn get_pitch(&self) -> f32 {
        self.controller.pitch
    }

    pub fn clean_up(&mut self, scene: &mut Scene) {
        scene.physics.remove_body(self.rigid_body);
        scene.remove_node(self.pivot);
    }
}

async fn create_skybox(resource_manager: ResourceManager) -> SkyBox {
    // Load skybox textures in parallel.
    let (front, back, left, right, top, bottom) = rg3d::futures::join!(
        resource_manager.request_texture("data/textures/skybox/front.jpg"),
        resource_manager.request_texture("data/textures/skybox/back.jpg"),
        resource_manager.request_texture("data/textures/skybox/left.jpg"),
        resource_manager.request_texture("data/textures/skybox/right.jpg"),
        resource_manager.request_texture("data/textures/skybox/up.jpg"),
        resource_manager.request_texture("data/textures/skybox/down.jpg")
    );

    // Unwrap everything.
    let skybox = SkyBox {
        front: Some(front.unwrap()),
        back: Some(back.unwrap()),
        left: Some(left.unwrap()),
        right: Some(right.unwrap()),
        top: Some(top.unwrap()),
        bottom: Some(bottom.unwrap()),
    };

    // Set S and T coordinate wrap mode, ClampToEdge will remove any possible seams on edges
    // of the skybox.
    for skybox_texture in skybox.textures().iter().filter_map(|t| t.clone()) {
        let mut data = skybox_texture.data_ref();
        data.set_s_wrap_mode(TextureWrapMode::ClampToEdge);
        data.set_t_wrap_mode(TextureWrapMode::ClampToEdge);
    }

    skybox
}

#[cfg(not(feature = "server"))]
fn create_bullet_impact(
    graph: &mut Graph,
    resource_manager: ResourceManager,
    pos: Vector3<f32>,
    orientation: UnitQuaternion<f32>,
) -> Handle<Node> {
    // Create sphere emitter first.
    let emitter = SphereEmitterBuilder::new(
        BaseEmitterBuilder::new()
            .with_max_particles(200)
            .with_spawn_rate(1000)
            .with_size_modifier_range(NumericRange::new(-0.01, -0.0125))
            .with_size_range(NumericRange::new(0.0010, 0.01))
            .with_x_velocity_range(NumericRange::new(-0.01, 0.01))
            .with_y_velocity_range(NumericRange::new(0.017, 0.02))
            .with_z_velocity_range(NumericRange::new(-0.01, 0.01))
            .resurrect_particles(false),
    )
    .with_radius(0.01)
    .build();

    // Color gradient will be used to modify color of each particle over its lifetime.
    let color_gradient = {
        let mut gradient = ColorGradient::new();
        gradient.add_point(GradientPoint::new(0.00, Color::from_rgba(255, 255, 0, 0)));
        gradient.add_point(GradientPoint::new(0.05, Color::from_rgba(255, 160, 0, 255)));
        gradient.add_point(GradientPoint::new(0.95, Color::from_rgba(255, 120, 0, 255)));
        gradient.add_point(GradientPoint::new(1.00, Color::from_rgba(255, 60, 0, 0)));
        gradient
    };

    // Create new transform to orient and position particle system.
    let transform = TransformBuilder::new()
        .with_local_position(pos)
        .with_local_rotation(orientation)
        .build();

    // Finally create particle system with limited lifetime.
    ParticleSystemBuilder::new(
        BaseBuilder::new()
            .with_lifetime(1.0)
            .with_local_transform(transform),
    )
    .with_acceleration(Vector3::new(0.0, -10.0, 0.0))
    .with_color_over_lifetime_gradient(color_gradient)
    .with_emitters(vec![emitter])
    // We'll use simple spark texture for each particle.
    .with_texture(resource_manager.request_texture(Path::new("data/textures/spark.png")))
    .build(graph)
}

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
    let shape = Arc::new(RwLock::new(SurfaceSharedData::make_cylinder(
        6,    // Count of sides
        1.0,  // Radius
        1.0,  // Height
        true, // No caps are needed.
        // Rotate vertical cylinder around X axis to make it face towards Z axis
        UnitQuaternion::from_axis_angle(&Vector3::x_axis(), 90.0f32.to_radians()).to_homogeneous(),
    )));

    MeshBuilder::new(
        BaseBuilder::new()
            .with_local_transform(transform)
            .with_lifetime(0.05),
    )
    .with_surfaces(vec![SurfaceBuilder::new(shape)
        .with_color(Color::from_rgba(105, 171, 195, 150))
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
