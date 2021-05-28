pub mod level;
mod message;
mod player;

use crate::{level::Level, message::Message, player::Player};
use rg3d::{
    core::{
        algebra::{Isometry3, Translation3, UnitQuaternion, Vector3},
        color::Color,
        color_gradient::{ColorGradient, GradientPoint},
        math::ray::Ray,
        numeric_range::NumericRange,
        pool::{Handle, Pool},
    },
    engine::{resource_manager::ResourceManager, Engine},
    event::{DeviceEvent, ElementState, Event, MouseButton, VirtualKeyCode, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    gui::node::StubNode,
    physics::{dynamics::RigidBodyBuilder, geometry::ColliderBuilder},
    renderer::surface::{SurfaceBuilder, SurfaceSharedData},
    resource::texture::TextureWrapMode,
    scene::{
        base::BaseBuilder,
        camera::{CameraBuilder, SkyBox},
        graph::Graph,
        mesh::{MeshBuilder, RenderPath},
        node::Node,
        particle_system::{BaseEmitterBuilder, ParticleSystemBuilder, SphereEmitterBuilder},
        physics::RayCastOptions,
        transform::TransformBuilder,
        ColliderHandle, RigidBodyHandle, Scene,
    },
    window::{Fullscreen, WindowBuilder},
};
use std::{
    path::Path,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, RwLock,
    },
    thread, time,
};

// Create our own engine type aliases. These specializations are needed, because the engine
// provides a way to extend UI with custom nodes and messages.
type GameEngine = Engine<(), StubNode>;

// Our game logic will be updated at 60 Hz rate.
const TIMESTEP: f32 = 1.0 / 60.0;
const SERVER_ADDRESS: &str = "127.0.0.1:12351";
const CLIENT_ADDRESS: &str = "127.0.0.1:12352";

fn main() {
    const SERVER: bool = cfg!(feature = "server");

    // Configure main window first.
    let window_builder = WindowBuilder::new()
        .with_visible(!SERVER)
        .with_title("Breakfloor")
        .with_fullscreen(Some(Fullscreen::Borderless(None)));

    // Create event loop that will be used to "listen" events from the OS.
    let event_loop = EventLoop::new();

    // Finally create an instance of the engine.
    let mut engine = GameEngine::new(window_builder, &event_loop, true).unwrap();

    engine
        .renderer
        .set_quality_settings(&rg3d::renderer::QualitySettings {
            use_ssao: false,
            ..Default::default()
        })
        .unwrap();

    if !SERVER {
        let window = engine.get_window();
        window.set_cursor_visible(false);
        let _ = window.set_cursor_grab(true);
    }

    // Move this to a Game module or something
    let (sender, receiver) = mpsc::channel();

    // Load level
    let mut level =
        rg3d::futures::executor::block_on(Level::new(&mut engine, "block_scene", receiver));

    rg3d::futures::executor::block_on(level.spawn_player(
        &mut engine,
        0,
        Vector3::new(0.0, 1.0, 0.0),
        true,
    ));

    rg3d::futures::executor::block_on(level.spawn_player(
        &mut engine,
        1,
        Vector3::new(0.0, 1.0, 1.0),
        false,
    ));

    // Run the event loop of the main window. which will respond to OS and window events and update
    // engine's state accordingly. Engine lets you to decide which event should be handled,
    // this is minimal working example if how it should be.
    let clock = time::Instant::now();
    let mut elapsed_time = 0.0;
    event_loop.run(move |event, _, control_flow| {
        // if game.focused {
        process_input_event(&event, sender.clone());
        // }

        match event {
            Event::MainEventsCleared => {
                // This main game loop - it has fixed time step which means that game
                // code will run at fixed speed even if renderer can't give you desired
                // 60 fps.
                let mut dt = clock.elapsed().as_secs_f32() - elapsed_time;
                while dt >= TIMESTEP {
                    dt -= TIMESTEP;
                    elapsed_time += TIMESTEP;

                    // Run our game's logic.
                    level.update(&mut engine, TIMESTEP);

                    // Update engine each frame.
                    engine.update(TIMESTEP);
                }

                // Rendering must be explicitly requested and handled after RedrawRequested event is received.
                engine.get_window().request_redraw();
            }
            Event::RedrawRequested(_) => {
                // Render at max speed - it is not tied to the game code.
                engine.render(TIMESTEP).unwrap();
            }
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                WindowEvent::KeyboardInput { input, .. } => {
                    // Exit game by hitting Escape.
                    if let Some(VirtualKeyCode::Escape) = input.virtual_keycode {
                        *control_flow = ControlFlow::Exit
                    }
                }
                WindowEvent::Resized(size) => {
                    // It is very important to handle Resized event from window, because
                    // renderer knows nothing about window size - it must be notified
                    // directly when window size has changed.
                    engine.renderer.set_frame_size(size.into());
                }
                // WindowEvent::Focused(focus) => {
                //     game.focused = focus;
                // }
                _ => (),
            },
            _ => *control_flow = ControlFlow::Poll,
        }
    });
}

fn process_input_event(event: &Event<()>, sender: Sender<Message>) {
    let player_index = 0;

    match event {
        Event::WindowEvent { event, .. } => match event {
            WindowEvent::KeyboardInput { input, .. } => {
                if let Some(key_code) = input.virtual_keycode {
                    match key_code {
                        VirtualKeyCode::W => {
                            sender
                                .send(Message::MoveForward {
                                    index: player_index,
                                    active: input.state == ElementState::Pressed,
                                })
                                .unwrap();
                        }
                        VirtualKeyCode::S => {
                            sender
                                .send(Message::MoveBackward {
                                    index: player_index,
                                    active: input.state == ElementState::Pressed,
                                })
                                .unwrap();
                        }
                        VirtualKeyCode::A => {
                            sender
                                .send(Message::MoveLeft {
                                    index: player_index,
                                    active: input.state == ElementState::Pressed,
                                })
                                .unwrap();
                        }
                        VirtualKeyCode::D => {
                            sender
                                .send(Message::MoveRight {
                                    index: player_index,
                                    active: input.state == ElementState::Pressed,
                                })
                                .unwrap();
                        }
                        VirtualKeyCode::Space => {
                            sender
                                .send(Message::MoveUp {
                                    index: player_index,
                                    active: input.state == ElementState::Pressed,
                                })
                                .unwrap();
                        }
                        _ => (),
                    }
                }
            }
            &WindowEvent::MouseInput { button, state, .. } => {
                if button == MouseButton::Left {
                    sender
                        .send(Message::ShootWeapon {
                            index: player_index,
                            active: state == ElementState::Pressed,
                        })
                        .unwrap();
                }
            }
            _ => {}
        },
        Event::DeviceEvent { event, .. } => {
            if let DeviceEvent::MouseMotion { delta } = event {
                let mouse_sens = 0.5;

                sender
                    .send(Message::LookAround {
                        index: player_index,
                        yaw_delta: mouse_sens * delta.0 as f32,
                        pitch_delta: mouse_sens * delta.1 as f32,
                    })
                    .unwrap();
            }
        }
        _ => (),
    }
}
