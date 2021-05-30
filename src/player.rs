use bincode::serialize;
use crossbeam_channel::Sender;
use laminar::Packet;
use rg3d::{
    core::{
        algebra::{Isometry, Transform3, Translation3, UnitQuaternion, Vector3},
        color::Color,
        color_gradient::{ColorGradient, GradientPoint},
        math::{ray::Ray, Matrix3Ext, Matrix4Ext, Vector3Ext},
        numeric_range::NumericRange,
        pool::Handle,
    },
    engine::resource_manager::ResourceManager,
    event::ElementState,
    physics::{
        dynamics::{RigidBody, RigidBodyBuilder},
        geometry::ColliderBuilder,
    },
    renderer::surface::{SurfaceBuilder, SurfaceSharedData},
    resource::texture::TextureWrapMode,
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
};
use std::{
    net::SocketAddr,
    path::Path,
    sync::{Arc, RwLock},
};

use crate::{
    message::{ActionMessage, NetworkMessage},
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
    pub shoot: bool,
    pub new_state: Option<PlayerState>,
    pub previous_state: PlayerState,
}

pub struct Player {
    pivot: Handle<Node>,
    weapon_pivot: Handle<Node>,
    spine: Handle<Node>,
    camera: Handle<Node>,
    rigid_body: RigidBodyHandle,
    collider: ColliderHandle,
    shot_timer: f32,
    recoil_offset: Vector3<f32>,
    recoil_target_offset: Vector3<f32>,
    pub index: u32,
    pub controller: PlayerController,
}

#[derive(Default)]

pub struct PlayerState {
    pub position: Vector3<f32>,
    pub velocity: f32,
    pub yaw: f32,
    pub pitch: f32,
}

impl Player {
    pub async fn new(
        scene: &mut Scene,
        // transform: Transform3<f32>
        position: Vector3<f32>,
        resource_manager: ResourceManager,
        current_player: bool,
        index: u32,
    ) -> Self {
        let first_person_model = resource_manager
            .request_model("data/models/shooting.fbx")
            .await
            .unwrap()
            .instantiate_geometry(scene);

        let third_person_model = resource_manager
            .request_model("data/models/shooting.fbx")
            .await
            .unwrap()
            .instantiate_geometry(scene);

        let camera_pos = Vector3::new(0.0, 0.37, 0.0);
        let model_pos = Vector3::new(0.0, -0.82, -0.07);

        scene.graph[first_person_model]
            .local_transform_mut()
            .set_position(model_pos)
            .set_scale(Vector3::new(0.1, 0.1, 0.1));

        scene.graph[third_person_model]
            .local_transform_mut()
            .set_position(model_pos + camera_pos)
            .set_scale(Vector3::new(0.1, 0.1, 0.1));

        // Show models for first person or third person
        let soldier = scene.graph.find_by_name(first_person_model, "soldier_LOD0");
        scene.graph[soldier].set_visibility(!current_player);
        scene.graph[third_person_model].set_visibility(!current_player);
        scene.graph[first_person_model].set_visibility(current_player);

        // Workaround for gun getting culled
        let gun = scene.graph.find_by_name(first_person_model, "gun_LOD0");
        scene.graph[gun]
            .local_transform_mut()
            .set_position(Vector3::new(0.0, 0.82, 0.5));

        let spine = scene.graph.find_by_name(third_person_model, "Bind_Spine");

        let weapon_pivot = scene
            .graph
            .find_by_name(first_person_model, "Bind_LeftHandIndex2");

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
        .with_skybox(create_skybox(resource_manager).await)
        .build(&mut scene.graph);

        let pivot = BaseBuilder::new()
            .with_children(&[camera, third_person_model])
            .with_tag("player".to_string()) // TODO: Use collider groups instead
            .build(&mut scene.graph);

        // Create rigid body, it will be used for interaction with the world.
        let rigid_body = scene.physics.add_body(
            RigidBodyBuilder::new_dynamic()
                .lock_rotations() // We don't want a bot to tilt.
                .translation(position.x, position.y, position.z) // Set desired position.
                .gravity_scale(GRAVITY_SCALE)
                .build(),
        );

        // Add capsule collider for the rigid body.
        let collider = scene
            .physics
            .add_collider(ColliderBuilder::capsule_y(0.25, 0.20).build(), rigid_body);

        // Bind pivot with rigid body. Scene will automatically sync transform of the pivot
        // with the transform of the rigid body.
        scene.physics_binder.bind(pivot, rigid_body);

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
            controller: Default::default(),
        }
    }

    pub fn update(
        &mut self,
        dt: f32,
        scene: &mut Scene,
        resource_manager: ResourceManager,
        packet_sender: &Sender<Packet>,
        client_address: &mut String,
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

        if self.controller.move_up {
            body.apply_impulse(pivot.up_vector() * JET_SPEED, true);
        }

        // Change the rotation of the rigid body according to current yaw. These lines responsible for
        // left-right rotation.
        let mut position = *body.position();
        position.rotation =
            UnitQuaternion::from_axis_angle(&Vector3::y_axis(), self.controller.yaw.to_radians());

        body.set_position(position, true);

        self.interpolate_state(body, &mut velocity);

        // Set pitch for the camera. These lines responsible for up-down camera rotation.

        scene.graph[self.camera].local_transform_mut().set_rotation(
            UnitQuaternion::from_axis_angle(&Vector3::x_axis(), self.controller.pitch.to_radians()),
        );

        scene.graph[self.spine].local_transform_mut().set_rotation(
            UnitQuaternion::from_axis_angle(&Vector3::x_axis(), self.controller.pitch.to_radians()),
        );

        if self.controller.shoot {
            self.shoot_weapon(scene, resource_manager, packet_sender, client_address);
        }
    }

    fn interpolate_state(&mut self, body: &mut RigidBody, velocity: &mut Vector3<f32>) {
        if let Some(new_state) = &self.controller.new_state {
            let distance = new_state
                .position
                .sqr_distance(&self.controller.previous_state.position);

            let error = (distance / MOVEMENT_SPEED).clamp(0.0, 1.0);

            if error > 0.0 {
                let mut pos = *body.position();
                // TODO: WHY IS THIS NOT UPDATING
                pos = pos.lerp_slerp(
                    &Isometry::from_parts(
                        Translation3::new(
                            new_state.position.x,
                            new_state.position.y,
                            new_state.position.z,
                        ),
                        pos.rotation,
                    ),
                    error,
                );
                body.set_position(pos, true);
                self.controller.previous_state.position = pos.translation.vector;
                self.controller.new_state = None;

                println!(
                    "interpolated new position based on error of {}: {:?}",
                    error, pos
                );
            }
        }
    }

    pub fn can_shoot(&self) -> bool {
        self.shot_timer <= 0.0
    }

    fn shoot_weapon(
        &mut self,
        scene: &mut Scene,
        resource_manager: ResourceManager,
        packet_sender: &Sender<Packet>,
        client_address: &mut String,
    ) {
        if self.can_shoot() {
            self.shot_timer = 0.1;

            self.recoil_target_offset = Vector3::new(0.0, 0.0, -0.035);

            let mut intersections = Vec::new();

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
                // TODO: Move this to a network module that can handle getting the address and sending etc

                let collider = scene
                    .physics
                    .colliders
                    .get(intersection.collider.into())
                    .unwrap();

                let body = collider.parent();

                if let Some(node_handle) = scene.physics_binder.node_of(body.into()) {
                    let node = &mut scene.graph[node_handle];
                    let tag = node.tag().clone();

                    let mut destroy_block = false;

                    // TODO: Should probably use collider groups instead of tag?
                    match tag {
                        "wall" => (),
                        "player" => (),
                        "0" => {
                            destroy_block = true;
                        }
                        _ => {
                            if cfg!(feature = "server") {
                                node.set_tag("0".to_string())
                            }
                        }
                    }

                    if destroy_block {
                        if let Ok(client_addr) = client_address.parse::<SocketAddr>() {
                            let message = NetworkMessage::Action {
                                index: node_handle.index(), // TODO: Probably need a different kind of message for GameMessage and PlayerMessage?
                                action: ActionMessage::DestroyBlock {
                                    index: node_handle.index(),
                                },
                            };

                            println!("sent destroyed block message");

                            packet_sender
                                .send(Packet::reliable_ordered(
                                    client_addr,
                                    serialize(&message).unwrap(),
                                    None,
                                ))
                                .unwrap();
                        }

                        scene.remove_node(node_handle);
                        scene.physics.remove_body(body.into());
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

            create_shot_trail(&mut scene.graph, ray.origin, ray.dir, trail_length);

            // Reset camera rotation
            // scene.graph[self.camera]
            //     .local_transform_mut()
            //     .set_rotation(original_rotation);
        }
    }

    pub fn get_vertical_velocity(&self, scene: &Scene) -> f32 {
        let body = scene.physics.bodies.get(self.rigid_body.into()).unwrap();

        body.linvel().y
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
        .with_local_scale(Vector3::new(0.01, 0.01, trail_length))
        // Rotate the trail along given `direction`
        .with_local_rotation(UnitQuaternion::face_towards(&direction, &Vector3::y()))
        .build();

    // Create unit cylinder with caps that faces toward Z axis.
    let shape = Arc::new(RwLock::new(SurfaceSharedData::make_cylinder(
        6,     // Count of sides
        1.0,   // Radius
        1.0,   // Height
        false, // No caps are needed.
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
