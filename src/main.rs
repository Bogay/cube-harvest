use askama::Template;
use core::panic;
use k8s_openapi::api::core::v1::Node;
use k8s_openapi::api::core::v1::Pod;
use kube::api::PostParams;
use kube::{Api, Client, Config, api::ListParams};
use macroquad::experimental::collections::storage;
use macroquad::prelude::{
    animation::{AnimatedSprite, Animation},
    *,
};
use macroquad_particles::{self, AtlasConfig, Emitter, EmitterConfig};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
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

enum GameStage {
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

#[derive(Template, Debug)]
#[template(path = "astro-unit.json", escape = "none")]
struct AstroUnitTemplate {
    name: String,
    target_ip: String,
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

#[derive(Debug, Clone)]
enum NavigationMode {
    Cluster,
    Node,
    Create,
}

#[derive(Debug, Clone)]
enum CreateTarget {
    Miner,
    Processor,
}

#[derive(Debug, Clone)]
struct GameState {
    selected_node_index: usize,
    navigation_mode: NavigationMode,
    create_target: Option<CreateTarget>,
    create_text_buf: String,
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
    let (k_tx, mut k_rx) = mpsc::channel(0x20);

    // TODO: handle exiting game
    let reconciliation_loop = tokio::spawn(async move {
        loop {
            let game_resources = GameResources::new(&client).await;
            tx.send(GameMessage::UpdateResources(game_resources))
                .await
                .expect("failed to send game msg");
            match k_rx.try_recv() {
                Ok(msg) => match msg {
                    GameMessage::CreatePod(pod) => {
                        let api = Api::default_namespaced(client.clone());
                        api.create(&PostParams::default(), &pod)
                            .await
                            .expect("failed to create pod");
                    }
                    GameMessage::UpdateResources(_) => unreachable!(),
                },
                Err(err) => {
                    if !matches!(err, mpsc::error::TryRecvError::Empty) {
                        panic!("{err}");
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    // Because macroquad need to be executed on one thread, we open it
    // from tokio main function
    // ref: https://github.com/not-fl3/macroquad/issues/182#issuecomment-1001571263
    let game_window_handle = open_game_window(rx, k_tx);

    game_window_handle.await.unwrap();
    reconciliation_loop.await.unwrap();
}

enum GameMessage {
    UpdateResources(GameResources),
    CreatePod(Pod),
}

fn open_game_window(rx: Receiver<GameMessage>, k_tx: Sender<GameMessage>) -> JoinHandle<()> {
    tokio::task::spawn_blocking(|| {
        macroquad::Window::from_config(
            Conf {
                sample_count: 4,
                window_title: "CubeHarvest: Cluster Frontier".to_string(),
                high_dpi: true,
                ..Default::default()
            },
            draw(rx, k_tx),
        );
    })
}

async fn draw(mut rx: Receiver<GameMessage>, k_tx: Sender<GameMessage>) {
    rand::srand(miniquad::date::now() as u64);
    set_pc_assets_folder("assets");

    storage::store(GameState {
        selected_node_index: 0,
        navigation_mode: NavigationMode::Cluster,
        create_target: None,
        create_text_buf: "".to_string(),
    });

    let mut explosions: Vec<(Emitter, Vec2)> = vec![];
    let mut game_stage = GameStage::MainMenu;
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
                    GameMessage::CreatePod(_) => unreachable!(),
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

        match game_stage {
            GameStage::MainMenu => {
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
                    game_stage = GameStage::Playing;
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
            GameStage::Playing => {
                // update
                let delta_time = get_frame_time();
                let mut game_state = storage::get_mut::<GameState>().clone();
                score = {
                    let game_resources = storage::get::<GameResources>();
                    game_resources.pods.len() as u32
                };
                let nodes_len = {
                    let game_resources = storage::get::<GameResources>();
                    game_resources.nodes.len()
                };

                match game_state.navigation_mode {
                    NavigationMode::Cluster => {
                        if is_key_pressed(KeyCode::Right) {
                            game_state.selected_node_index =
                                game_state.selected_node_index.saturating_add(1);
                        }
                        if is_key_pressed(KeyCode::Left) {
                            game_state.selected_node_index =
                                game_state.selected_node_index.saturating_sub(1);
                        }
                        if is_key_pressed(KeyCode::Enter) {
                            game_state.navigation_mode = NavigationMode::Node;
                        }
                    }
                    NavigationMode::Node => {
                        if is_key_pressed(KeyCode::Escape) {
                            game_state.navigation_mode = NavigationMode::Cluster;
                        }

                        if is_key_pressed(KeyCode::C) {
                            game_state.navigation_mode = NavigationMode::Create;
                            game_state.create_text_buf.clear();
                        }

                        if is_key_pressed(KeyCode::D) {
                            // TODO: delete selected unit
                        }
                        if is_key_pressed(KeyCode::Right) {
                            // TODO: update unit selection
                        }
                        if is_key_pressed(KeyCode::Left) {
                            // TODO: update unit selection
                        }
                    }
                    NavigationMode::Create => match &game_state.create_target {
                        None => {
                            if is_key_pressed(KeyCode::Escape) {
                                game_state.navigation_mode = NavigationMode::Node;
                            }

                            if is_key_pressed(KeyCode::M) {
                                game_state.create_target = Some(CreateTarget::Miner);
                            }
                            if is_key_pressed(KeyCode::P) {
                                game_state.create_target = Some(CreateTarget::Processor);
                            }
                        }
                        Some(target) => {
                            if is_key_pressed(KeyCode::Enter) {
                                let unit_id = rand::rand();
                                let astro_unit = AstroUnitTemplate {
                                    name: format!("miner-{unit_id}"),
                                    target_ip: (rand::rand() % 255).to_string(),
                                }
                                .render()
                                .unwrap();
                                let astro_unit = serde_json::from_str::<Pod>(&astro_unit)
                                    .expect("failed to parse astro unit json");
                                println!("Create {target:?} -> {}", game_state.create_text_buf);
                                k_tx.send(GameMessage::CreatePod(astro_unit))
                                    .await
                                    .expect("failed to send pod");
                                game_state.navigation_mode = NavigationMode::Node;
                            } else if is_key_pressed(KeyCode::Escape) {
                                game_state.navigation_mode = NavigationMode::Node;
                            } else if let Some(c) = get_char_pressed() {
                                if c.is_digit(10) {
                                    game_state.create_text_buf.push(c);
                                }
                            }
                        }
                    },
                }

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
                game_state.selected_node_index =
                    clamp(game_state.selected_node_index, 0, nodes_len - 1);

                // post update
                storage::store(game_state);

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
                draw_top_panel();

                // draw node
                draw_node();

                // draw navbar
                draw_navbar();

                // post draw
                // bullets.retain(|b| b.y > 0. - b.size / 2.);
                // squares.retain(|s| !s.collided);
                // bullets.retain(|b| !b.collided);
                // explosions.retain(|(e, _)| e.config.emitting);
            }
            GameStage::Paused => {
                if is_key_pressed(KeyCode::Space) {
                    game_stage = GameStage::Playing;
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
            GameStage::GameOver => {
                if is_key_pressed(KeyCode::Space) {
                    game_stage = GameStage::MainMenu;
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

fn draw_top_panel() {
    let game_state = storage::get::<GameState>();

    let label_size = 25;
    let label_scale = 1.0;
    let label_padding = 4.0;
    let label_dimensions = measure_text("Placeholder", None, label_size, label_scale);
    draw_text(
        &format!("Astro Units: {}", 1), // TODO: fetch pods
        10.0,
        35.0,
        label_size as f32,
        WHITE,
    );
    draw_text(
        &format!("Credits    : {}", 2), // TODO: fetch credits
        10.0,
        35.0 + (label_dimensions.height + label_padding) * 2.,
        label_size as f32,
        WHITE,
    );
    draw_text(
        &format!("Astro Node : {}", game_state.selected_node_index),
        10.0,
        35.0 + label_dimensions.height + label_padding,
        label_size as f32,
        WHITE,
    );
}

fn draw_node() {
    let width = screen_width();
    let height = screen_height();
    let node_index = storage::get::<GameState>().selected_node_index;
    let game_resources = storage::get::<GameResources>();
    let node = &game_resources.nodes[node_index];
    let node_name = node.metadata.name.as_ref().expect("nodes should have name");
    let pods = game_resources
        .pods
        .iter()
        .filter(|p| {
            p.spec
                .as_ref()
                .and_then(|s| s.node_name.as_ref())
                .map(|nn| nn == node_name)
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    // draw node plane
    let node_width = width * 0.7;
    let node_height = 100.;
    draw_rectangle(
        width / 2. - node_width / 2.,
        height - node_height / 2.,
        node_width,
        node_height,
        WHITE,
    );

    // draw pods info
    let pod_size = 32.;
    for (i, p) in pods.iter().enumerate() {
        draw_astro_unit(
            width / 2. - 200. + pod_size * 1.5 * i as f32,
            height - node_height / 2. + 15. - pod_size / 2.,
            pod_size,
            BLUE,
        );
    }
    // draw_text(&format!("{}", pods.len()), 0., height - 10., 18., WHITE);
}

fn draw_astro_unit(x: f32, y: f32, size: f32, color: Color) {
    // Main body (simple rectangle or custom polygon)
    draw_rectangle(x - size / 2.0, y - size / 2.0, size, size, color);

    // Optional: a small "engine" or "sensor" part
    draw_triangle(
        vec2(x - size / 4.0, y + size / 2.0),
        vec2(x + size / 4.0, y + size / 2.0),
        vec2(x, y + size / 2.0 + size / 4.0),
        GRAY,
    );
}

fn draw_navbar() {
    let width = screen_width();
    let height = screen_height();
    let navigation_mode = storage::get::<GameState>().navigation_mode.clone();

    let label_font_size = 18;
    let label_dim = measure_text("Cluster", None, label_font_size, 1.);
    let padding = 4.;

    // draw navbar background
    draw_rectangle(
        0.,
        height - padding * 2. - label_dim.height - 5.,
        width,
        padding * 2. + label_dim.height + 10.,
        GRAY,
    );

    // draw tooltip
    let mut tooltip = String::with_capacity(0x50);
    match navigation_mode {
        NavigationMode::Cluster => {
            tooltip.push_str("Cluster");
            tooltip.push_str(" | [Enter] Select node");
            tooltip.push_str(" | [<- ->] Switch node");
        }
        NavigationMode::Node => {
            tooltip.push_str("Node   ");
            tooltip.push_str(" | [Esc] Back");
            tooltip.push_str(" | [<- ->] Switch unit");
            tooltip.push_str(" | [D]elete unit");
            tooltip.push_str(" | [C]reate unit");
        }
        NavigationMode::Create => {
            tooltip.push_str("Create ");
            let game_state = storage::get::<GameState>();
            match game_state.create_target.as_ref() {
                Some(target) => {
                    tooltip.push_str(" | ");
                    tooltip.push_str(&format!("{target:?}"));
                    tooltip.push_str(" : ");
                    tooltip.push_str(&game_state.create_text_buf);
                }
                None => {
                    tooltip.push_str(" | [Esc] Back");
                    tooltip.push_str(" | [M]iner");
                    tooltip.push_str(" | [P]rocessor");
                }
            }
        }
    }
    draw_text(
        &tooltip,
        0. + padding,
        height - label_dim.height / 2. - padding,
        18.,
        WHITE,
    );
}
