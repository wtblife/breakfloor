#![cfg_attr(not(feature = "console"), windows_subsystem = "windows")]
pub mod game;
pub mod level;
pub mod network_manager;
pub mod player;
pub mod player_event;

use crate::{
    game::Game,
    level::Level,
    network_manager::{NetworkManager, NetworkMessage},
    player::Player,
    player_event::PlayerEvent,
};
use crossbeam_channel::{Receiver, Sender};
use laminar::{Config, ErrorKind, Packet, Socket, SocketEvent};
use player::PlayerState;
use rg3d::{
    core::{
        algebra::{Isometry3, Translation3, UnitQuaternion, Vector2, Vector3},
        color::Color,
        color_gradient::{ColorGradient, GradientPoint},
        math::ray::Ray,
        numeric_range::NumericRange,
        pool::{Handle, Pool},
        profiler::print,
    },
    engine::{framework::UiNode, resource_manager::ResourceManager, Engine},
    event::{DeviceEvent, ElementState, Event, MouseButton, VirtualKeyCode, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    gui::{
        grid::GridBuilder,
        message::{MessageDirection, TextMessage},
        node::StubNode,
        scroll_bar::ScrollBarBuilder,
        text::TextBuilder,
        widget::WidgetBuilder,
        VerticalAlignment,
    },
    physics::{dynamics::RigidBodyBuilder, geometry::ColliderBuilder},
    scene::{
        base::BaseBuilder,
        camera::{CameraBuilder, SkyBox},
        graph::Graph,
        mesh::{MeshBuilder, RenderPath},
        node::Node,
        physics::RayCastOptions,
        transform::TransformBuilder,
    },
    window::{Fullscreen, WindowBuilder},
};
use serde::Deserialize;
use std::{
    fmt,
    net::{SocketAddr, ToSocketAddrs},
    os::windows::process,
    path::Path,
    sync::{
        mpsc::{self},
        Arc, RwLock,
    },
    thread,
    time::{self, Duration, Instant},
};

// Create our own engine type aliases. These specializations are needed, because the engine
// provides a way to extend UI with custom nodes and messages.
type GameEngine = Engine<(), StubNode>;

use std::error::Error;
use std::fs::File;
use std::io::BufReader;

#[derive(Deserialize, Debug)]
#[serde(default)]
pub struct Settings {
    look_sensitivity: f32,
    vsync: bool,
    fullscreen: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            look_sensitivity: 0.5,
            vsync: false,
            fullscreen: false,
        }
    }
}

fn read_settings_from_file<P: AsRef<Path>>(path: P) -> Result<Settings, Box<dyn Error>> {
    // Open the file in read-only mode with buffer.
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    // Read the JSON contents of the file as an instance of `User`.
    let u = serde_json::from_reader(reader)?;

    // Return the `User`.
    Ok(u)
}

fn main() {
    const SERVER: bool = cfg!(feature = "server");
    // Our game logic will be updated at 60 Hz rate.
    const TIMESTEP: f32 = 1.0 / 60.0;

    let settings: Settings = read_settings_from_file("settings.json").unwrap_or_default();
    let fullscreen = if settings.fullscreen {
        Some(Fullscreen::Borderless(None))
    } else {
        None
    };

    // Configure main window first.
    let window_builder = WindowBuilder::new()
        .with_visible(!SERVER)
        .with_title("Breakfloor")
        .with_fullscreen(fullscreen);

    // Create event loop that will be used to "listen" events from the OS.
    let event_loop = EventLoop::new();

    // Finally create an instance of the engine.
    let mut engine = GameEngine::new(window_builder, &event_loop, settings.vsync).unwrap();

    engine
        .renderer
        .set_quality_settings(&rg3d::renderer::QualitySettings {
            use_ssao: false,
            ..Default::default()
        })
        .unwrap();

    let mut interface = create_ui(&mut engine);

    #[cfg(not(feature = "server"))]
    {
        let window = engine.get_window();
        window.set_cursor_visible(false);
        let _ = window.set_cursor_grab(true);
    }

    // Run the event loop of the main window. which will respond to OS and window events and update
    // engine's state accordingly. Engine lets you to decide which event should be handled,
    // this is minimal working example if how it should be.
    let clock = time::Instant::now();
    let mut elapsed_time = 0.0;
    let mut focused = true;
    let mut cursor_in_window = true;

    let mut network_manager = NetworkManager::new();
    let mut game = rg3d::core::futures::executor::block_on(Game::new(&mut engine, settings));

    event_loop.run(move |event, _, control_flow| {
        network_manager.handle_events(&mut engine, &mut game);

        #[cfg(not(feature = "server"))]
        if focused && cursor_in_window {
            process_input_event(&event, &mut game, &mut network_manager);
        }

        match event {
            Event::MainEventsCleared => {
                // This main game loop - it has fixed time step which means that game
                // code will run at fixed speed even if renderer can't give you desired
                // 60 fps.
                let mut dt = clock.elapsed().as_secs_f32() - elapsed_time;
                while dt >= TIMESTEP {
                    dt -= TIMESTEP;
                    elapsed_time += TIMESTEP;

                    let fps = engine.renderer.get_statistics().frames_per_second;
                    #[cfg(not(feature = "server"))]
                    engine.user_interface.send_message(TextMessage::text(
                        interface.fps,
                        MessageDirection::ToWidget,
                        format!("FPS: {}", fps),
                    ));

                    // Run our game's logic.
                    game.update(
                        &mut engine,
                        TIMESTEP,
                        &mut network_manager,
                        elapsed_time,
                        &interface,
                    );

                    // Update engine each frame.
                    engine.update(TIMESTEP);
                }

                while let Some(ui_message) = engine.user_interface.poll_message() {
                    match ui_message.data() {
                        _ => (),
                    }
                }

                // Rendering must be explicitly requested and handled after RedrawRequested event is received.
                engine.get_window().request_redraw();
            }
            #[cfg(not(feature = "server"))]
            Event::RedrawRequested(_) => {
                // Render at max speed - it is not tied to the game code.
                engine.render().unwrap();
            }
            #[cfg(not(feature = "server"))]
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                WindowEvent::KeyboardInput { input, .. } => {
                    if focused && cursor_in_window {
                        // Exit game by hitting Escape.
                        if let Some(VirtualKeyCode::Escape) = input.virtual_keycode {
                            *control_flow = ControlFlow::Exit
                        }
                    }
                }
                WindowEvent::Resized(size) => {
                    // It is very important to handle Resized event from window, because
                    // renderer knows nothing about window size - it must be notified
                    // directly when window size has changed.
                    engine.renderer.set_frame_size(size.into());
                    // interface = create_ui(&mut engine);
                }
                WindowEvent::Focused(focus) => {
                    focused = focus;
                }
                WindowEvent::CursorEntered { device_id } => {
                    cursor_in_window = true;
                }
                WindowEvent::CursorLeft { device_id } => {
                    cursor_in_window = false;
                }
                _ => (),
            },
            _ => *control_flow = ControlFlow::Poll,
        }

        #[cfg(not(feature = "server"))]
        if !game.active {
            *control_flow = ControlFlow::Exit
        }
    });
}

#[cfg(not(feature = "server"))]
fn process_input_event(event: &Event<()>, game: &mut Game, network_manager: &mut NetworkManager) {
    if let (Some(player_index), Some(level)) = (network_manager.player_index, &mut game.level) {
        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::KeyboardInput { input, .. } => {
                    if let Some(key_code) = input.virtual_keycode {
                        match key_code {
                            VirtualKeyCode::W => {
                                if let Some(player) = level.get_player_by_index(player_index) {
                                    let action = PlayerEvent::MoveForward {
                                        index: player_index,
                                        active: input.state == ElementState::Pressed,
                                        yaw: player.get_yaw(),
                                        pitch: player.get_pitch(),
                                    };
                                    let message = NetworkMessage::PlayerEvent {
                                        index: player_index,
                                        event: action,
                                    };

                                    // TODO: Should active = false be reliable since it's only sent once?
                                    network_manager.send_to_server_unreliably(&message, 0);
                                    level.queue_event(action);
                                }
                            }
                            VirtualKeyCode::S => {
                                if let Some(player) = level.get_player_by_index(player_index) {
                                    let action = PlayerEvent::MoveBackward {
                                        index: player_index,
                                        active: input.state == ElementState::Pressed,
                                        yaw: player.get_yaw(),
                                        pitch: player.get_pitch(),
                                    };

                                    let message = NetworkMessage::PlayerEvent {
                                        index: player_index,
                                        event: action,
                                    };

                                    network_manager.send_to_server_unreliably(&message, 0);
                                    level.queue_event(action);
                                }
                            }
                            VirtualKeyCode::A => {
                                if let Some(player) = level.get_player_by_index(player_index) {
                                    let action = PlayerEvent::MoveLeft {
                                        index: player_index,
                                        active: input.state == ElementState::Pressed,
                                        yaw: player.get_yaw(),
                                        pitch: player.get_pitch(),
                                    };
                                    let message = NetworkMessage::PlayerEvent {
                                        index: player_index,
                                        event: action,
                                    };

                                    network_manager.send_to_server_unreliably(&message, 0);
                                    level.queue_event(action);
                                }
                            }
                            VirtualKeyCode::D => {
                                if let Some(player) = level.get_player_by_index(player_index) {
                                    let action = PlayerEvent::MoveRight {
                                        index: player_index,
                                        active: input.state == ElementState::Pressed,
                                        yaw: player.get_yaw(),
                                        pitch: player.get_pitch(),
                                    };
                                    let message = NetworkMessage::PlayerEvent {
                                        index: player_index,
                                        event: action,
                                    };

                                    network_manager.send_to_server_unreliably(&message, 0);
                                    level.queue_event(action);
                                }
                            }
                            VirtualKeyCode::Space => {
                                if let Some(player) = level.get_player_by_index(player_index) {
                                    let action = PlayerEvent::MoveUp {
                                        index: player_index,
                                        active: input.state == ElementState::Pressed,
                                        fuel: player.flight_fuel,
                                    };
                                    let message = NetworkMessage::PlayerEvent {
                                        index: player_index,
                                        event: action,
                                    };

                                    network_manager.send_to_server_unreliably(&message, 0);
                                    // level.queue_event(action);
                                }
                            }
                            _ => (),
                        }
                    }
                }
                &WindowEvent::MouseInput { button, state, .. } => {
                    if button == MouseButton::Left {
                        if let Some(player) = level.get_player_by_index(player_index) {
                            let message = NetworkMessage::PlayerEvent {
                                index: player_index,
                                event: PlayerEvent::ShootWeapon {
                                    index: player_index,
                                    active: state == ElementState::Pressed,
                                    yaw: player.get_yaw(),
                                    pitch: player.get_pitch(),
                                },
                            };

                            network_manager.send_to_server_reliably(&message);
                        }
                    }
                }
                _ => {}
            },
            Event::DeviceEvent { event, .. } => {
                if let DeviceEvent::MouseMotion { delta } = event {
                    let mouse_sens = game.settings.look_sensitivity;

                    let action = PlayerEvent::LookAround {
                        index: player_index,
                        yaw_delta: mouse_sens * delta.0 as f32,
                        pitch_delta: mouse_sens * delta.1 as f32,
                    };

                    let message = NetworkMessage::PlayerEvent {
                        index: player_index,
                        event: action,
                    };

                    network_manager.send_to_server_unreliably(&message, 0);
                    level.queue_event(action);
                }
            }
            _ => (),
        }
    }
}

pub struct Interface {
    fps: Handle<UiNode>,
    fuel: Handle<UiNode>,
}

fn create_ui(engine: &mut GameEngine) -> Interface {
    let window_width = engine.renderer.get_frame_size().0 as f32;
    let window_height = engine.renderer.get_frame_size().1 as f32;

    let ctx = &mut engine.user_interface.build_ctx();

    // First of all create debug text that will show title of example and current FPS.
    let fps = TextBuilder::new(WidgetBuilder::new()).build(ctx);
    let fuel = TextBuilder::new(
        WidgetBuilder::new()
            .with_desired_position(Vector2::new(window_width - 100.0, window_height - 25.0)),
    )
    .build(ctx);

    Interface { fps, fuel }
}
