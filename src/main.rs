use core::panic;
use k8s_openapi::api::core::v1::Node;
use k8s_openapi::api::core::v1::Pod;
use kube::{Api, Client, Config, api::ListParams};
use macroquad::experimental::collections::storage;
use macroquad::prelude::{
    animation::{AnimatedSprite, Animation},
    *,
};
use macroquad_particles::{self, AtlasConfig, ColorCurve, Emitter, EmitterConfig};
use std::fs;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio::task::JoinHandle;

const MOVEMENT_SPEED: f32 = 200.;
const FRAGMENT_SHADER: &str = include_str!("starfield-shader.glsl");
const VERTEX_SHADER: &str = "#version 100

attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
varying float iTime;

uniform mat4 Model;
uniform mat4 Projection;
uniform vec4 _Time;

void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    iTime = _Time.x;
}
";

enum GameState {
    MainMenu,
    Playing,
    Paused,
    GameOver,
}

struct Shape {
    size: f32,
    speed: f32,
    x: f32,
    y: f32,
    collided: bool,
}

struct GameResources {
    pods: Vec<Pod>,
    nodes: Vec<Node>,
}

impl GameResources {
    // TODO: error handling
    pub async fn new(client: &Client) -> Self {
        let list_params = ListParams::default();
        let pods = Api::default_namespaced(client.clone())
            .list(&list_params)
            .await
            .expect("failed to get pods");
        let nodes = Api::all(client.clone())
            .list(&list_params)
            .await
            .expect("failed to get nodes");

        Self {
            pods: pods.items,
            nodes: nodes.items,
        }
    }
}

impl Shape {
    fn collides_with(&self, other: &Self) -> bool {
        self.rect().overlaps(&other.rect())
    }

    fn rect(&self) -> Rect {
        Rect {
            x: self.x - self.size / 2.,
            y: self.y - self.size / 2.,
            w: self.size,
            h: self.size,
        }
    }
}

fn particle_explosion() -> EmitterConfig {
    EmitterConfig {
        local_coords: false,
        one_shot: true,
        emitting: true,
        lifetime: 0.6,
        lifetime_randomness: 0.3,
        explosiveness: 0.65,
        initial_direction_spread: 2. + std::f32::consts::PI,
        initial_velocity: 400.,
        initial_velocity_randomness: 0.8,
        size: 16.,
        size_randomness: 0.3,
        atlas: Some(AtlasConfig::new(5, 1, 0..)),
        ..Default::default()
    }
}

#[tokio::main]
async fn main() {
    // setup kube client
    let config = Config::infer().await.expect("failed to load kubeconfig");
    let client = Client::try_from(config).expect("failed to create kube client");
    let game_resources = GameResources::new(&client).await;
    let (tx, rx) = mpsc::channel(0x20);
    tx.send(GameMessage::UpdateResources(game_resources))
        .await
        .expect("failed to send game msg");

    let reconciliation_loop = tokio::spawn(async move {
        loop {
            let game_resources = GameResources::new(&client).await;
            tx.send(GameMessage::UpdateResources(game_resources))
                .await
                .expect("failed to send game msg");
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    // Because macroquad need to be executed on one thread, we open it
    // from tokio main function
    // ref: https://github.com/not-fl3/macroquad/issues/182#issuecomment-1001571263
    let game_window_handle = open_game_window(rx);

    reconciliation_loop.await.unwrap();
    game_window_handle.await.unwrap();
}

enum GameMessage {
    UpdateResources(GameResources),
}

fn open_game_window(rx: Receiver<GameMessage>) -> JoinHandle<()> {
    tokio::task::spawn_blocking(|| {
        macroquad::Window::from_config(
            Conf {
                sample_count: 4,
                window_title: "CubeHarvest: Cluster Frontier".to_string(),
                high_dpi: true,
                ..Default::default()
            },
            draw(rx),
        );
    })
}

async fn draw(mut rx: Receiver<GameMessage>) {
    rand::srand(miniquad::date::now() as u64);
    set_pc_assets_folder("assets");

    let mut explosions: Vec<(Emitter, Vec2)> = vec![];
    let mut game_state = GameState::MainMenu;
    let mut score: u32 = 0;
    let mut squares: Vec<Shape> = vec![];
    let mut bullets: Vec<Shape> = vec![];
    let mut circle = Shape {
        size: 32.,
        speed: MOVEMENT_SPEED,
        x: screen_width() / 2.,
        y: screen_height() / 2.,
        collided: false,
    };
    let mut direction_modifier: f32 = 0.;
    let render_target = render_target(320, 150);
    render_target.texture.set_filter(FilterMode::Nearest);
    let material = load_material(
        ShaderSource::Glsl {
            vertex: VERTEX_SHADER,
            fragment: FRAGMENT_SHADER,
        },
        MaterialParams {
            uniforms: vec![
                UniformDesc::new("iResolution", UniformType::Float2),
                UniformDesc::new("direction_modifier", UniformType::Float1),
            ],
            ..Default::default()
        },
    )
    .unwrap();
    let ship_texture = load_texture("ship.png")
        .await
        .expect("failed to load ship image");
    ship_texture.set_filter(FilterMode::Nearest);
    let bullet_texture = load_texture("laser-bolts.png")
        .await
        .expect("failed to load laser image");
    bullet_texture.set_filter(FilterMode::Nearest);
    let explosion_texture = load_texture("explosion.png")
        .await
        .expect("failed to load explosion image");
    explosion_texture.set_filter(FilterMode::Nearest);
    let enemy_small_texture = load_texture("enemy-small.png")
        .await
        .expect("failed to load enemy image");
    enemy_small_texture.set_filter(FilterMode::Nearest);
    // call after loading all textures
    build_textures_atlas();

    let mut bullet_sprite = AnimatedSprite::new(
        16,
        16,
        &[
            Animation {
                name: "bullet".to_string(),
                row: 0,
                frames: 2,
                fps: 12,
            },
            Animation {
                name: "bolt".to_string(),
                row: 1,
                frames: 2,
                fps: 12,
            },
        ],
        true,
    );
    bullet_sprite.set_animation(1);
    let mut ship_sprite = AnimatedSprite::new(
        16,
        24,
        &[
            Animation {
                name: "idle".to_string(),
                row: 0,
                frames: 2,
                fps: 12,
            },
            Animation {
                name: "left".to_string(),
                row: 2,
                frames: 2,
                fps: 12,
            },
            Animation {
                name: "right".to_string(),
                row: 4,
                frames: 2,
                fps: 12,
            },
        ],
        true,
    );
    let mut enemy_small_sprite = AnimatedSprite::new(
        17,
        16,
        &[Animation {
            name: "enemy_small".to_string(),
            row: 0,
            frames: 2,
            fps: 12,
        }],
        true,
    );

    // game loop
    loop {
        clear_background(BLACK);

        // consume messages
        loop {
            match rx.try_recv() {
                Ok(msg) => match msg {
                    GameMessage::UpdateResources(game_resources) => storage::store(game_resources),
                },
                Err(err) => {
                    if matches!(err, mpsc::error::TryRecvError::Empty) {
                        break;
                    }
                    panic!("{err}");
                }
            }
        }

        material.set_uniform("iResolution", (screen_width(), screen_height()));
        material.set_uniform("direction_modifier", direction_modifier);
        gl_use_material(&material);
        draw_texture_ex(
            &render_target.texture,
            0.,
            0.,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(screen_width(), screen_height())),
                ..Default::default()
            },
        );
        gl_use_default_material();

        match game_state {
            GameState::MainMenu => {
                // update
                if is_key_pressed(KeyCode::Escape) {
                    std::process::exit(0);
                }

                if is_key_pressed(KeyCode::Space) {
                    squares.clear();
                    bullets.clear();
                    explosions.clear();
                    circle.x = screen_width() / 2.;
                    circle.y = screen_height() / 2.;
                    score = 0;
                    game_state = GameState::Playing;
                }

                // draw
                let text = "Press space";
                let text_dimestions = measure_text(text, None, 50, 1.);
                draw_text(
                    text,
                    screen_width() / 2. - text_dimestions.width / 2.,
                    screen_height() / 2.,
                    50.,
                    WHITE,
                );
            }
            GameState::Playing => {
                // update
                let delta_time = get_frame_time();
                let game_resources = storage::get::<GameResources>();
                score = game_resources.pods.len() as u32;
                // if rand::gen_range(0, 99) >= 95 {
                //     let size = rand::gen_range(16., 64.);
                //     squares.push(Shape {
                //         size,
                //         speed: rand::gen_range(50., 150.),
                //         x: rand::gen_range(size / 2., screen_width() - size / 2.),
                //         y: -size,
                //         collided: false,
                //     });
                // }
                // for sq in &mut squares {
                //     sq.y += sq.speed * delta_time;
                // }
                if is_key_down(KeyCode::Escape) {
                    game_state = GameState::Paused;
                }

                // if is_key_down(KeyCode::Space) {
                //     bullets.push(Shape {
                //         x: circle.x,
                //         y: circle.y - 24.,
                //         speed: circle.speed * 2.,
                //         size: 32.,
                //         collided: false,
                //     });
                // }
                // for bullet in &mut bullets {
                //     bullet.y -= bullet.speed * delta_time;
                // }

                // if is_key_down(KeyCode::Right) {
                //     circle.x += MOVEMENT_SPEED * delta_time;
                //     direction_modifier += 0.05 * delta_time;
                //     ship_sprite.set_animation(2);
                // }
                // if is_key_down(KeyCode::Left) {
                //     circle.x -= MOVEMENT_SPEED * delta_time;
                //     direction_modifier -= 0.05 * delta_time;
                //     ship_sprite.set_animation(1);
                // }
                // if is_key_down(KeyCode::Down) {
                //     circle.y += MOVEMENT_SPEED * delta_time;
                // }
                // if is_key_down(KeyCode::Up) {
                //     circle.y -= MOVEMENT_SPEED * delta_time;
                // }
                // circle.x = clamp(circle.x, 0.0, screen_width());
                // circle.y = clamp(circle.y, 0.0, screen_height());

                // for sq in &mut squares {
                //     for bullet in &mut bullets {
                //         if bullet.collides_with(sq) {
                //             bullet.collided = true;
                //             sq.collided = true;

                //             score += sq.size.round() as u32;
                //             high_score = high_score.max(score);

                //             explosions.push((
                //                 Emitter::new(EmitterConfig {
                //                     amount: sq.size.round() as u32 * 2,
                //                     texture: Some(explosion_texture.clone()),
                //                     ..particle_explosion()
                //                 }),
                //                 vec2(sq.x, sq.y),
                //             ));
                //         }
                //     }
                // }

                // if squares.iter().any(|s| circle.collides_with(s)) {
                //     if score == high_score {
                //         fs::write("highscore.dat", high_score.to_string())
                //             .expect("failed to record high score.");
                //     }
                //     game_state = GameState::GameOver;
                // }

                // draw enemy
                // enemy_small_sprite.update();
                // let enemy_frame = enemy_small_sprite.frame();
                // for sq in &squares {
                //     draw_texture_ex(
                //         &enemy_small_texture,
                //         sq.x - sq.size / 2.,
                //         sq.y - sq.size / 2.,
                //         WHITE,
                //         DrawTextureParams {
                //             dest_size: Some(vec2(sq.size, sq.size)),
                //             source: Some(enemy_frame.source_rect),
                //             ..Default::default()
                //         },
                //     );
                // }
                // for (e, coords) in &mut explosions {
                //     e.draw(*coords);
                // }
                // draw spaceship
                // ship_sprite.update();
                // let ship_frame = ship_sprite.frame();
                // draw_texture_ex(
                //     &ship_texture,
                //     circle.x - circle.size / 2.,
                //     circle.y - circle.size / 2.,
                //     WHITE,
                //     DrawTextureParams {
                //         dest_size: Some(vec2(circle.size, circle.size)),
                //         source: Some(ship_frame.source_rect),
                //         ..Default::default()
                //     },
                // );
                // draw bullets
                // bullet_sprite.update();
                // let bullet_frame = bullet_sprite.frame();
                // for bullet in &bullets {
                //     draw_texture_ex(
                //         &bullet_texture,
                //         bullet.x - bullet.size / 2.,
                //         bullet.y - bullet.size / 2.,
                //         WHITE,
                //         DrawTextureParams {
                //             dest_size: Some(vec2(bullet.size, bullet.size)),
                //             source: Some(bullet_frame.source_rect),
                //             ..Default::default()
                //         },
                //     );
                // }
                draw_text(&format!("Score: {}", score), 10.0, 35.0, 25.0, WHITE);

                // post draw
                // bullets.retain(|b| b.y > 0. - b.size / 2.);
                // squares.retain(|s| !s.collided);
                // bullets.retain(|b| !b.collided);
                // explosions.retain(|(e, _)| e.config.emitting);
            }
            GameState::Paused => {
                if is_key_pressed(KeyCode::Space) {
                    game_state = GameState::Playing;
                }

                let text = "Paused";
                let text_dimensions = measure_text(text, None, 50, 1.);
                draw_text(
                    text,
                    screen_width() / 2. - text_dimensions.width / 2.,
                    screen_height() / 2.,
                    50.,
                    WHITE,
                );
            }
            GameState::GameOver => {
                if is_key_pressed(KeyCode::Space) {
                    game_state = GameState::MainMenu;
                }

                let text = "GAME OVER!";
                let text_dimensions = measure_text(text, None, 50, 1.);
                draw_text(
                    text,
                    screen_width() / 2. - text_dimensions.width / 2.,
                    screen_height() / 2.,
                    50.,
                    RED,
                );
            }
        };

        next_frame().await
    }
}
