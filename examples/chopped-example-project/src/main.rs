use crate::{
    config::GameConfig,
    editor::EditMode,
    game::{GameEvents, GameLogic, MoveInputState},
    level_manager::LevelManager,
    sprites::preload_sprites,
};
use chopped_engine::kiss3d::{egui, prelude::*, window};
use chopped_asset_handler::fetch_asset_bytes;
use chopped_engine::rodio::Source;
use chopped_engine::timestepper::FixedTimeStepper;

mod config;
mod editor;
mod game;
mod level;
mod level_manager;
mod sprite_list;
mod sprites;
/// How much of the player sprite the status text is allowed to fill, so it doesn't run
/// right up against the sprite's edges.
const TEXT_FIT_IN_SPRITE: f32 = 0.9;

async fn load_config() -> GameConfig {
    let config_bytes = fetch_asset_bytes(config::CONFIG_PATH)
        .await
        .expect("should be able to fetch config.ron");
    ron::de::from_bytes(&config_bytes).expect("config.ron should be valid RON")
}

async fn create_game_logic_from_config(
    level_manager: &mut LevelManager,
    root_scene: &mut SceneNode2d,
    texture_manager: &mut TextureManager,
) -> GameLogic {
    let config = load_config().await;
    let level = level_manager.load_current_level().await;
    log!("Loading level {}", level_manager.current_level_name());

    GameLogic::new(config, level, root_scene, texture_manager)
}

async fn create_game_logic_and_data(
    level_manager: &mut LevelManager,
    root_scene: &mut SceneNode2d,
    texture_manager: &mut TextureManager,
) -> GameLogic {
    create_game_logic_from_config(level_manager, root_scene, texture_manager).await
}

/// Finds the largest uniform font scale that keeps `text` inside a `max_size` box of screen
/// pixels, and returns it along with the size the text ends up taking, so the caller can
/// place the box wherever it wants.
///
/// rusttype's metrics are linear in the scale, so we measure the string once at scale 1.0
/// and divide rather than searching for a fit.
fn fit_text_to_box(font: &Font, text: &str, max_size: Vec2) -> (f32, Vec2) {
    let unit_scale = chopped_engine::rusttype::Scale::uniform(1.0);
    let v_metrics = font.font().v_metrics(unit_scale);
    let unit_height = v_metrics.ascent - v_metrics.descent;
    // the width of the whole run is where the last glyph starts plus how far it advances
    let unit_width = font
        .font()
        .layout(text, unit_scale, chopped_engine::rusttype::Point { x: 0., y: 0. })
        .last()
        .map(|glyph| glyph.position().x + glyph.unpositioned().h_metrics().advance_width)
        .unwrap_or(0.);

    let height_limited_scale = max_size.y / unit_height;
    let scale = if unit_width > 0. {
        (max_size.x / unit_width).min(height_limited_scale)
    } else {
        height_limited_scale
    };

    (scale, Vec2::new(unit_width, unit_height) * scale)
}

fn camera_to_screen_position(position: Vec2, camera: &PanZoomCamera2d, window: &Window) -> Vec2 {
    (position - camera.at()) * camera.zoom() * Vec2::new(1., -1.) + window.size().as_vec2() / 2.
}

const FINISHED_TEXT: &str = "YOU FINISHED! Press J to play again.";

#[kiss3d::main]
async fn main() {
    #[cfg(target_arch = "wasm32")]
    {
        console_error_panic_hook::set_once();
    }
    let mut window = Window::new("Kiss3d: rectangle").await;
    let mut texture_manager = TextureManager::new();

    let mut camera = PanZoomCamera2d::new(Vec2::ZERO, 5.0);
    let mut root_scene = SceneNode2d::empty();

    // preload all sprites into texture manager
    preload_sprites("sprites.ron", &mut texture_manager).await;

    let mut level_manager = LevelManager::new("level_list.ron").await;
    let mut game_logic =
        create_game_logic_and_data(&mut level_manager, &mut root_scene, &mut texture_manager).await;
    let mut time_stepper = FixedTimeStepper::default();

    let font = Font::default();

    // input state
    let mut input = MoveInputState::default();

    // start song
    let device_sink = rodio::DeviceSinkBuilder::open_default_sink()
        .expect("should be able to open the default audio device");
    let rocket_player = rodio::Player::connect_new(device_sink.mixer());
    let rocket_file = fetch_asset_bytes("rocket-thrust-effect.wav")
        .await
        .expect("should have sound");
    let rocket_sound = rodio::Decoder::new(std::io::Cursor::new(rocket_file))
        .expect("sound should be decodable")
        .repeat_infinite();
    // queue it once and keep it silent until the player thrusts
    rocket_player.append(rocket_sound);
    rocket_player.pause();

    // finished logic
    let mut finished = false;
    while window.render_2d(&mut root_scene, &mut camera).await {
        // the way OS's poll key inputs mean that there's a frame of waiting before sending in the next key input
        // see: https://stereopsis.com/keyrepeat/
        for event in window.events().iter() {
            match event.value {
                WindowEvent::Key(Key::R, Action::Press, _) => {
                    log!("Reload game!");
                    let reloaded_game = create_game_logic_and_data(
                        &mut level_manager,
                        &mut root_scene,
                        &mut texture_manager,
                    )
                    .await;
                    game_logic = reloaded_game;
                }
                WindowEvent::Key(Key::S, Action::Press, _) => {
                    cfg_select! {
                        target_arch = "wasm32" => {
                            log!("Can't do this on WASM!");
                        },
                        _ => {
                            if game_logic.get_edit_mode() != EditMode::None {
                                log!("save level");
                                game_logic
                                    .create_level_from_game_logic()
                                    .write_level_to_disc(level_manager.current_level_name())
                                    .expect("level should be saved");
                            } else {
                                log!("can't do this when not in edit mode");
                            }
                        }
                    }
                }
                WindowEvent::Key(Key::E, Action::Press, _) => {
                    game_logic.set_edit_mode(EditMode::Move);
                }
                WindowEvent::Key(Key::F, Action::Press, _) => {
                    game_logic.set_edit_mode(EditMode::Scale);
                }
                WindowEvent::Key(Key::P, Action::Press, _) => {
                    game_logic.set_edit_mode(EditMode::None);
                }
                WindowEvent::Key(Key::T, Action::Press, _) => {
                    game_logic.set_current_selected_as_first_spawn();
                }
                WindowEvent::Key(Key::N, Action::Press, _) => {
                    if let Some((x, y)) = window.cursor_pos() {
                        let screen_position = Vec2::new(x as f32, y as f32);
                        let world_position =
                            camera.unproject(screen_position, window.size().as_vec2());
                        game_logic.create_or_change_type_of_selected(
                            &mut root_scene,
                            &mut texture_manager,
                            world_position,
                        );
                    }
                }
                WindowEvent::Key(Key::Delete, Action::Press, _) => {
                    game_logic.delete_selected();
                }
                WindowEvent::Key(Key::Up, action, _) => match action {
                    Action::Release => input.up = false,
                    Action::Press => input.up = true,
                },
                WindowEvent::Key(Key::Down, action, _) => match action {
                    Action::Release => input.down = false,
                    Action::Press => input.down = true,
                },
                WindowEvent::Key(Key::Left, action, _) => match action {
                    Action::Release => input.left = false,
                    Action::Press => input.left = true,
                },
                WindowEvent::Key(Key::Right, action, _) => match action {
                    Action::Release => input.right = false,
                    Action::Press => input.right = true,
                },
                WindowEvent::Key(Key::Space, action, _) => match action {
                    Action::Release => input.thrust = false,
                    Action::Press => input.thrust = true,
                },
                WindowEvent::Key(Key::J, Action::Press, _) => {
                    finished = false;
                    level_manager.load_level_by_index(0).await;
                    log!("Restart game!");
                    let advanced_game = create_game_logic_and_data(
                        &mut level_manager,
                        &mut root_scene,
                        &mut texture_manager,
                    )
                    .await;
                    game_logic = advanced_game;
                }
                WindowEvent::Key(Key::Numpad1, Action::Press, _) => {
                    if game_logic.get_edit_mode() != EditMode::None {
                        level_manager.load_level_by_index(0).await;
                        log!("Restart game!");
                        let advanced_game = create_game_logic_and_data(
                            &mut level_manager,
                            &mut root_scene,
                            &mut texture_manager,
                        )
                        .await;
                        game_logic = advanced_game;
                    }
                }
                WindowEvent::Key(Key::Numpad2, Action::Press, _) => {
                    if game_logic.get_edit_mode() != EditMode::None {
                        level_manager.load_level_by_index(1).await;
                        log!("Restart game!");
                        let advanced_game = create_game_logic_and_data(
                            &mut level_manager,
                            &mut root_scene,
                            &mut texture_manager,
                        )
                        .await;
                        game_logic = advanced_game;
                    }
                }
                WindowEvent::Key(Key::Numpad3, Action::Press, _) => {
                    if game_logic.get_edit_mode() != EditMode::None {
                        level_manager.load_level_by_index(2).await;
                        log!("Restart game!");
                        let advanced_game = create_game_logic_and_data(
                            &mut level_manager,
                            &mut root_scene,
                            &mut texture_manager,
                        )
                        .await;
                        game_logic = advanced_game;
                    }
                }
                WindowEvent::Key(Key::Numpad4, Action::Press, _) => {
                    if game_logic.get_edit_mode() != EditMode::None {
                        level_manager.load_level_by_index(3).await;
                        log!("Restart game!");
                        let advanced_game = create_game_logic_and_data(
                            &mut level_manager,
                            &mut root_scene,
                            &mut texture_manager,
                        )
                        .await;
                        game_logic = advanced_game;
                    }
                }
                WindowEvent::Key(Key::Numpad5, Action::Press, _) => {
                    if game_logic.get_edit_mode() != EditMode::None {
                        level_manager.load_level_by_index(4).await;
                        log!("Restart game!");
                        let advanced_game = create_game_logic_and_data(
                            &mut level_manager,
                            &mut root_scene,
                            &mut texture_manager,
                        )
                        .await;
                        game_logic = advanced_game;
                    }
                }
                WindowEvent::Key(Key::Numpad6, Action::Press, _) => {
                    if game_logic.get_edit_mode() != EditMode::None {
                        level_manager.load_level_by_index(5).await;
                        log!("Restart game!");
                        let advanced_game = create_game_logic_and_data(
                            &mut level_manager,
                            &mut root_scene,
                            &mut texture_manager,
                        )
                        .await;
                        game_logic = advanced_game;
                    }
                }
                WindowEvent::Key(Key::Numpad7, Action::Press, _) => {
                    if game_logic.get_edit_mode() != EditMode::None {
                        level_manager.load_level_by_index(6).await;
                        log!("Restart game!");
                        let advanced_game = create_game_logic_and_data(
                            &mut level_manager,
                            &mut root_scene,
                            &mut texture_manager,
                        )
                        .await;
                        game_logic = advanced_game;
                    }
                }
                WindowEvent::Key(Key::Numpad8, Action::Press, _) => {
                    if game_logic.get_edit_mode() != EditMode::None {
                        level_manager.load_level_by_index(7).await;
                        log!("Restart game!");
                        let advanced_game = create_game_logic_and_data(
                            &mut level_manager,
                            &mut root_scene,
                            &mut texture_manager,
                        )
                        .await;
                        game_logic = advanced_game;
                    }
                }
                WindowEvent::Key(Key::Numpad9, Action::Press, _) => {
                    if game_logic.get_edit_mode() != EditMode::None {
                        level_manager.load_level_by_index(8).await;
                        log!("Restart game!");
                        let advanced_game = create_game_logic_and_data(
                            &mut level_manager,
                            &mut root_scene,
                            &mut texture_manager,
                        )
                        .await;
                        game_logic = advanced_game;
                    }
                }

                WindowEvent::MouseButton(MouseButton::Button1, Action::Press, _) => {
                    if let Some((x, y)) = window.cursor_pos() {
                        let screen_position = Vec2::new(x as f32, y as f32);
                        let world_position =
                            camera.unproject(screen_position, window.size().as_vec2());
                        game_logic.check_object_at_world_position(world_position);
                    }
                }
                _ => {}
            }
        }
        // play sound if we have inputs (and stop if we don't)
        // we put it above the rest of the logic just so we don't get the audio repeating on the final screen
        let rocket_firing = game_logic.get_edit_mode() == EditMode::None
            && (input.left || input.right || input.thrust);
        if rocket_firing {
            rocket_player.play();
        } else {
            rocket_player.pause();
        }
        // we don't run any game logic when we are in finished
        if finished {
            let window_size = window.size().as_vec2();
            let (text_scale, text_size) =
                fit_text_to_box(&font, FINISHED_TEXT, window_size * TEXT_FIT_IN_SPRITE);
            // draw_text anchors the top left of the text, so center the text
            window.draw_text(
                FINISHED_TEXT,
                (window_size / 2.) - text_size / 2.,
                text_scale,
                &font,
                WHITE,
            );
            continue;
        }

        // run actual game logic if we've hit a tick (drain accumulated frames)
        let mut pending_event = GameEvents::None;
        while time_stepper.step() {
            // don't update velocity if input has no changes
            game_logic.update_position_with_input(&input);
            let game_event = game_logic.step();
            // an exit means we're leaving this level anyways, so we don't care about any more events
            if game_event == GameEvents::Exit {
                pending_event = GameEvents::Exit;
                break;
            }
        }
        // show the path each moving platform patrols along
        for (point_1, point_2) in game_logic.get_moving_platform_paths() {
            window.draw_line_2d(point_1, point_2, WHITE, 10.0);
        }

        // render sprites
        let (position, velocity) = game_logic.get_player_position_and_velocity();

        // center camera on player
        camera.look_at(position, camera.zoom());

        // draw text
        let speed_text = {
            let velocity_x = velocity.x.abs();
            let velocity_y = velocity.y.abs();
            let crash_speed_x = game_logic.get_config().player_speed_for_horizontal_crash;
            let crash_speed_y = game_logic.get_config().player_speed_for_vertical_crash;
            let warning_limit = 10.;
            if velocity_x > crash_speed_x || velocity_y > crash_speed_y {
                "TOO FAST"
            } else if velocity_x > (crash_speed_x - warning_limit).max(0.)
                || velocity_y > (crash_speed_y - warning_limit).max(0.)
            {
                "WARNING"
            } else {
                "Ok"
            }
        };
        // draw_text takes screen pixels (origin top-left, y down) but `position` is in world
        // space, so project it through the camera - the inverse of `camera.unproject`.
        let player_screen_position = camera_to_screen_position(position, &camera, &window);
        // the player's sprite covers this many screen pixels at the current zoom
        let player_screen_size = Vec2::new(
            game_logic.get_config().player_game_width,
            game_logic.get_config().player_game_height,
        ) * camera.zoom();
        let (text_scale, text_size) =
            fit_text_to_box(&font, speed_text, player_screen_size * TEXT_FIT_IN_SPRITE);
        // draw_text anchors the top left of the text, so center the text
        window.draw_text(
            speed_text,
            player_screen_position - text_size / 2.,
            text_scale,
            &font,
            BLACK,
        );

        for endpoint in game_logic.get_all_endpoints() {
            let endpoint_position =
                camera_to_screen_position(endpoint.get_position(), &camera, &window);
            window.draw_text("X", endpoint_position, 30., &font, WHITE);
        }

        // Draw EXIT on top of the exit
        let exit_rect = game_logic.get_exit_rectangle();
        let exit_position = camera_to_screen_position(exit_rect.get_position(), &camera, &window);
        let exit_screen_size = Vec2::new(exit_rect.width, exit_rect.height) * camera.zoom();
        let (text_scale, text_size) = fit_text_to_box(&font, "EXIT", exit_screen_size);
        // draw_text anchors the top left of the text, so center the text
        window.draw_text(
            "EXIT",
            exit_position - text_size / 2.,
            text_scale,
            &font,
            WHITE,
        );

        // we address pending events here after all the rendering is done
        if pending_event == GameEvents::Exit {
            if level_manager.is_on_last_level() {
                log!("You finished the final level");
                finished = true;
                // erase all level objects
                // note: we store all of the children in a vec first to prevent runtime panics
                // otherwise we would be freeing a child while they're still being borrowed
                // also this does invalidate all node scene keys, but its fine since we aren't calling game logic step anymore
                let children: Vec<SceneNode2d> = root_scene.data().children().to_owned();
                for mut child in children {
                    child.detach();
                }
            } else {
                level_manager.advance();
                log!("Advanced game!");
                let advanced_game = create_game_logic_and_data(
                    &mut level_manager,
                    &mut root_scene,
                    &mut texture_manager,
                )
                .await;
                game_logic = advanced_game;
            }
        }
    }
}
